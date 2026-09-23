//! The chores cargo has no subcommand for.
//!
//! `.cargo/config.toml` gives each one an alias, so `cargo clean-all`
//! reaches the `clean-all` arm below. A cargo alias can only name a
//! cargo subcommand, and `run` is the one that reaches our own code —
//! which is why these live in a crate rather than in a shell script the
//! alias cannot call.

use std::path::Path;
use std::process::ExitCode;

/// Every build output the repo ignores, relative to the repo root.
///
/// `cargo clean` reaches only the workspace it is run from, and this
/// repo has two: the root's target and the frontend's own, which is a
/// separate workspace because it targets wasm. Trunk's dist and the
/// backend's generated permission schemas are build outputs too.
const OUTPUTS: [&str; 4] = [
    "target",
    "crates/tauri/frontend/target",
    "crates/tauri/frontend/dist",
    "crates/tauri/backend/gen/schemas",
];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("clean-all") => clean_all(),
        Some(other) => {
            eprintln!("xtask: no such chore: {other}");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("xtask: which chore? (clean-all)");
            ExitCode::FAILURE
        }
    }
}

/// Remove every build output, and say what went.
///
/// Cargo runs this with the workspace root as the working directory, so
/// the paths above resolve against the repo whatever directory the
/// `cargo clean-all` was typed in.
fn clean_all() -> ExitCode {
    let mut code = ExitCode::SUCCESS;
    for output in OUTPUTS {
        let path = Path::new(output);
        if !path.exists() {
            continue;
        }
        println!("removing {output}");
        // A concurrent build recreates files as fast as they are
        // removed, so a failure here means something is still writing.
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) => {
                eprintln!("xtask: could not remove {output}: {error}");
                code = ExitCode::FAILURE;
            }
        }
    }
    code
}
