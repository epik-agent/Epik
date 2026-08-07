//! Chores a cargo alias cannot say alone; each is one word after `--`.
//!
//! `clean` — start completely over. Cargo's aliases run exactly one
//! subcommand, and this repository keeps build state in three places: the
//! root workspace's `target`, the `crates/epik-ui` workspace's own
//! `target` (its separateness is why a root `cargo clean` cannot reach
//! it), and Trunk's `dist`, which the Tauri host would otherwise serve
//! stale. Deleting `target` deletes this very binary mid-run, which the
//! operating systems Epik builds on are all fine with.
//!
//! `brand-sync` — ask GitHub whether `crates/epik-ui/brand/brand.json`
//! still matches the website's copy. The comparison deliberately goes to
//! the repository on GitHub, never to a sibling directory of this clone:
//! a clone's layout is nobody's contract.
//!
//! Paths are anchored to this crate's manifest rather than the current
//! directory, so a chore means the same thing from anywhere in the root
//! workspace.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

/// Everywhere build state hides, relative to the repository root.
const STATE: [&str; 3] = ["target", "crates/epik-ui/target", "crates/epik-ui/dist"];

/// The palette as the website publishes it, at the address GitHub serves
/// raw files from — the default branch, which is the one the website
/// deploys from.
const WEBSITE_BRAND: &str =
    "https://raw.githubusercontent.com/epik-agent/Epik/main/website/brand/brand.json";

/// The crate's own copy, relative to the repository root.
const CRATE_BRAND: &str = "crates/epik-ui/brand/brand.json";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("clean") => clean(),
        Some("brand-sync") => brand_sync(),
        chore => {
            let asked = chore.unwrap_or("nothing");
            eprintln!(
                "xtask knows the chores \"clean\" and \"brand-sync\"; it was asked {asked:?}"
            );
            ExitCode::FAILURE
        }
    }
}

/// The repository root: this crate's parent, wherever the clone lives.
fn root() -> Option<&'static Path> {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent()
}

fn clean() -> ExitCode {
    let Some(root) = root() else {
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

fn brand_sync() -> ExitCode {
    let Some(root) = root() else {
        eprintln!("xtask expects to sit one level under the repository root");
        return ExitCode::FAILURE;
    };
    let ours = match fs::read_to_string(root.join(CRATE_BRAND)) {
        Ok(ours) => ours,
        Err(error) => {
            eprintln!("could not read {CRATE_BRAND}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let theirs = match fetch(WEBSITE_BRAND) {
        Ok(theirs) => theirs,
        Err(error) => {
            eprintln!("could not fetch {WEBSITE_BRAND}: {error}");
            return ExitCode::FAILURE;
        }
    };
    if ours == theirs {
        println!("{CRATE_BRAND} agrees with the website's brand.json on GitHub");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "{CRATE_BRAND} has drifted from {WEBSITE_BRAND} — a retouch lands in both or has \
             not landed"
        );
        ExitCode::FAILURE
    }
}

fn fetch(url: &str) -> Result<String, ureq::Error> {
    ureq::get(url).call()?.body_mut().read_to_string()
}
