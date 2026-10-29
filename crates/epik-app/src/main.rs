//! The Epik app, which runs Epik's builds.

mod menu;
mod tray;
mod view;
mod window;

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
use epik_core::{record, run};

/// How often the app checks GitHub for runs whose epic issue closed or
/// whose pull request was closed. Modest to start with; tune it.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(180);

const USAGE: &str = "usage: epik-app [--headless | --version]";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("epik-app {}", epik_core::version());
            ExitCode::SUCCESS
        }
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
