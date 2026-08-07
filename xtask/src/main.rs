//! `cargo clean-all`: start completely over.
//!
//! A cargo alias runs exactly one subcommand, and this repository keeps
//! build state in three places — the root workspace's `target`, the
//! `crates/epik-ui` workspace's own `target` (its separateness is why a
//! root `cargo clean` cannot reach it), and Trunk's `dist`, which the
//! Tauri host would otherwise serve stale. The alias points here, and
//! this does the walking.
//!
//! Paths are anchored to this crate's manifest rather than the current
//! directory, so the command means the same thing from anywhere in the
//! root workspace. Deleting `target` deletes this very binary mid-run,
//! which the operating systems Epik builds on are all fine with.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

/// Everywhere build state hides, relative to the repository root.
const STATE: [&str; 3] = ["target", "crates/epik-ui/target", "crates/epik-ui/dist"];

fn main() -> ExitCode {
    let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() else {
        eprintln!("xtask expects to sit one level under the repository root");
        return ExitCode::FAILURE;
    };
    let mut failed = false;
    for state in STATE {
        let path = root.join(state);
        if !path.exists() {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => println!("removed {state}"),
            Err(error) => {
                eprintln!("could not remove {state}: {error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
