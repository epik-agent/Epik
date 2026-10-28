//! The Epik app, which runs Epik's builds.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use epik_core::awake::{self, Change, KeepAwake};
use epik_core::builds::Builds;
use epik_core::ipc::{Address, BindError, Server};
use epik_core::record;

const USAGE: &str = "usage: epik-app --headless | --version";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("epik-app {}", epik_core::version());
            ExitCode::SUCCESS
        }
        Some("--headless") => headless(),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Serves the socket and runs builds, without a desktop, until SIGINT or
/// SIGTERM (Ctrl-C on Windows); then cancels the builds still running and
/// exits.
fn headless() -> ExitCode {
    let Some(root) = record::default_root() else {
        eprintln!("epik-app: there is no home directory to keep Epik's runs in");
        return ExitCode::FAILURE;
    };
    let address = Address::in_root(&root);
    let server = match Server::bind(&address) {
        Ok(server) => server,
        Err(BindError::AlreadyRunning { version }) => {
            eprintln!("epik-app: an Epik app {version} is already running at {address}");
            return ExitCode::FAILURE;
        }
        Err(BindError::Io(error)) => {
            eprintln!("epik-app: couldn't listen at {address}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    if let Err(error) = ctrlc::set_handler({
        let stop = stop.clone();
        move || stop.store(true, Ordering::SeqCst)
    }) {
        eprintln!("epik-app: couldn't handle signals: {error}");
        return ExitCode::FAILURE;
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
    }));

    eprintln!("epik-app {}: serving at {address}", epik_core::version());
    server.serve(builds.clone(), &stop);

    let running = builds.active().len();
    eprintln!("epik-app: stopping, interrupting {running} build(s)");
    builds.shutdown();
    eprintln!("epik-app: stopped");
    ExitCode::SUCCESS
}
