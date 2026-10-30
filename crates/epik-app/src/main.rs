//! The Epik app, which runs Epik's builds.

mod main_window;
mod navigation;
mod pages;
mod popover;
mod setup_window;
mod tray;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use epik_core::awake::{self, Change, KeepAwake};
use epik_core::builds::Builds;
use epik_core::cleanup;
use epik_core::ipc::{Address, BindError, Server};
use epik_core::{record, run, self_link};

/// How often the app checks GitHub for runs whose epic issue closed or
/// whose pull request was closed. Modest to start with; tune it.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(180);

const USAGE: &str = "usage: epik-app [--headless | --version | mcp]";

fn main() -> ExitCode {
    find_epiks_tools();
    link_self();
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("epik-app {}", epik_core::version());
            ExitCode::SUCCESS
        }
        Some("mcp") => mcp(),
        Some("--headless") => match App::start(|_| {}) {
            Ok(app) => app.serve(),
            Err(code) => code,
        },
        None => tray::run(),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Serves Epik's MCP tools over stdin and stdout. Stdout is the protocol,
/// so everything else goes to stderr.
fn mcp() -> ExitCode {
    let Some(root) = record::default_root() else {
        eprintln!("epik-app: there is no home directory to keep Epik's runs in");
        return ExitCode::FAILURE;
    };
    let server = epik_core::mcp::Server::new(epik_core::mcp::Context {
        gh: epik_core::mcp::gh::Gh::new(),
        address: Address::in_root(&root),
        root,
        start_app: Box::new(start_detached),
        start_timeout: Duration::from_secs(15),
        plugin_dir: std::env::var_os("CLAUDE_PLUGIN_ROOT").map(PathBuf::from),
    });
    match server.serve_stdio() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("epik-app mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Starts this executable with no arguments, in the tray or headless,
/// detached so that it outlives the MCP server that started it.
fn start_detached() -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(std::env::current_exe()?);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().map(drop)
}

/// Puts Epik's private tools first on this process's `PATH`, with the
/// places installers put programs, so `gh`, `node` and `claude` are found
/// even when the app is opened from the desktop, and the agents it starts
/// find the same ones.
fn find_epiks_tools() {
    if let Some(root) = record::default_root() {
        let path = epik_core::setup::search_path(&root, std::env::var_os("PATH"));
        // SAFETY: called first in `main`, before any other thread exists
        // to read the environment.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// Points `~/.epik/bin/epik-app` at this executable, where Claude Code and
/// other programs find the app. A failure is only reported.
fn link_self() {
    let (Some(root), Ok(exe)) = (record::default_root(), std::env::current_exe()) else {
        return;
    };
    if let Err(error) = self_link::refresh(&root, &exe) {
        eprintln!(
            "epik-app: couldn't link {} to this app: {error}",
            self_link::path(&root).display()
        );
    }
}

/// The app's socket and builds, which run the same with a tray or without.
struct App {
    root: PathBuf,
    address: Address,
    server: Server,
    /// Set by SIGINT or SIGTERM (Ctrl-C on Windows), or by the tray's Quit:
    /// stops serving, and so stops the app.
    stop: Arc<AtomicBool>,
    builds: Arc<Builds>,
}

impl App {
    /// Binds the socket and readies the builds, keeping the machine awake
    /// while any run, and telling `on_active` too how many are running each
    /// time that changes. Says why on stderr if it can't.
    fn start(on_active: impl Fn(usize) + Send + Sync + 'static) -> Result<Self, ExitCode> {
        let Some(root) = record::default_root() else {
            eprintln!("epik-app: there is no home directory to keep Epik's runs in");
            return Err(ExitCode::FAILURE);
        };
        let address = Address::in_root(&root);
        let server = match Server::bind(&address) {
            Ok(server) => server,
            Err(BindError::AlreadyRunning { version }) => {
                eprintln!("epik-app: an Epik app {version} is already running at {address}");
                return Err(ExitCode::FAILURE);
            }
            Err(BindError::Io(error)) => {
                eprintln!("epik-app: couldn't listen at {address}: {error}");
                return Err(ExitCode::FAILURE);
            }
        };
        // Only now that this app holds the socket is no other app building,
        // so every clone left, and every build recorded as running, is
        // from a build that never ended.
        if let Err(error) = cleanup::sweep_builds(&root) {
            eprintln!("epik-app: couldn't remove leftover builds: {error}");
        }
        if let Err(error) = run::interrupt_unfinished(&root) {
            eprintln!("epik-app: couldn't record unfinished builds as interrupted: {error}");
        }
        let stop = Arc::new(AtomicBool::new(false));
        if let Err(error) = ctrlc::set_handler({
            let stop = stop.clone();
            move || stop.store(true, Ordering::SeqCst)
        }) {
            eprintln!("epik-app: couldn't handle signals: {error}");
            return Err(ExitCode::FAILURE);
        }
        let keep_awake = KeepAwake::new(awake::System);
        let builds = Arc::new(Builds::real(&root).on_active(move |active| {
            eprintln!("epik-app: {active} build(s) running");
            match keep_awake.follow(active) {
                Ok(Some(Change::Held)) => eprintln!("epik-app: keeping the machine awake"),
                Ok(Some(Change::Released)) => eprintln!("epik-app: letting the machine sleep"),
                Ok(None) => {}
                Err(error) => eprintln!("epik-app: couldn't keep the machine awake: {error}"),
            }
            on_active(active);
        }));
        Ok(Self {
            root,
            address,
            server,
            stop,
            builds,
        })
    }

    /// Serves the socket and runs builds until told to stop; then cancels
    /// the builds still running and waits for them.
    fn serve(self) -> ExitCode {
        eprintln!(
            "epik-app {}: serving at {}",
            epik_core::version(),
            self.address
        );
        let reconciler = thread::spawn({
            let builds = self.builds.clone();
            let stop = self.stop.clone();
            move || reconcile_until(&builds, &stop)
        });
        self.server.serve(self.builds.clone(), &self.stop);
        let _ = reconciler.join();

        let running = self.builds.active().len();
        eprintln!("epik-app: stopping, interrupting {running} build(s)");
        self.builds.shutdown();
        eprintln!("epik-app: stopped");
        ExitCode::SUCCESS
    }
}

/// Checks GitHub for the runs every [`RECONCILE_INTERVAL`], starting at
/// once, until `stop` is set, and says on stderr what each check did.
fn reconcile_until(builds: &Builds, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        let report = builds.reconcile();
        for run in &report.finished {
            eprintln!("epik-app: {run}'s issue closed, so its run is commented on and deleted");
        }
        for run in &report.needs_attention {
            eprintln!("epik-app: {run}'s pull request was closed without merging");
        }
        for repo in &report.missing {
            eprintln!("epik-app: GitHub doesn't find {repo}");
        }
        for error in &report.errors {
            eprintln!("epik-app: couldn't reconcile: {error}");
        }
        let until = Instant::now() + RECONCILE_INTERVAL;
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(200));
        }
    }
}
