//! The Epik app, which runs Epik's builds.

use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("epik-app {}", epik_core::version());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: epik-app --version");
            ExitCode::FAILURE
        }
    }
}
