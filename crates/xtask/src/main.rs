//! `cargo xtask`: the release workflow's macOS steps, runnable on a Mac.
//!
//! `cargo xtask dmg` builds the universal `.dmg` the way the release
//! workflow does and checks it; `cargo xtask check-dmg [path]` checks one
//! already built. `cargo xtask latest-json <version> <tag> <dir>` writes
//! the updater's manifest for the update files in `<dir>`.

mod dmg;
mod latest_json;

use std::process::ExitCode;

pub type Error = Box<dyn std::error::Error>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result: Result<(), Error> = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["dmg"] => dmg::build().and_then(|path| dmg::check(&path)),
        ["check-dmg"] => dmg::built().and_then(|path| dmg::check(&path)),
        ["check-dmg", path] => dmg::check(path.as_ref()),
        ["latest-json", version, tag, dir] => latest_json::write(version, tag, dir.as_ref()),
        _ => Err(
            "usage: cargo xtask dmg | check-dmg [path] | latest-json <version> <tag> <dir>".into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
