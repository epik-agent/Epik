//! A `claude` that is entirely script.
//!
//! The conformance staging copies this binary into a fresh directory as
//! `claude`, writes `feed.json` — a list of beats, each a pause in
//! milliseconds and a stream-json line — beside it, and points [`ClaudeCode`]
//! at the copy. The stub performs the feed exactly as canned: hostility is
//! the feed's business, not the stub's, so an overspending, stalling, or
//! posthumously chatty claude is just a feed that says so.
//!
//! It opens with the init handshake a real claude opens with, reporting its
//! cwd, argv, and one probe env var — which is how the tests see, from
//! inside the process, what the wrapper actually provisioned.
//!
//! [`ClaudeCode`]: epik::agent::ClaudeCode

use std::env;
use std::fs;
use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

/// A feed line spelled exactly so is performed as raw bytes that are not
/// UTF-8 — JSON strings cannot carry the real thing. Its twin lives in
/// `tests/claude_code.rs`.
const MANGLED: &str = "<mangled-utf8>";

fn main() -> Result<()> {
    let stub = env::current_exe().context("locating the stub")?;
    let feed = stub
        .parent()
        .context("the stub has no directory")?
        .join("feed.json");
    let text = fs::read_to_string(&feed)
        .with_context(|| format!("reading the feed at {}", feed.display()))?;
    let beats: Vec<(u64, String)> = serde_json::from_str(&text).context("decoding the feed")?;

    let init = serde_json::json!({
        "type": "system",
        "subtype": "init",
        "cwd": env::current_dir().ok(),
        "argv": env::args().collect::<Vec<_>>(),
        "probe": env::var("EPIK_STUB_PROBE").ok(),
    });
    println!("{init}");

    for (pause, line) in beats {
        thread::sleep(Duration::from_millis(pause));
        if line == MANGLED {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(b"\xff{ not utf-8 }\xff\n")?;
            stdout.flush()?;
        } else {
            println!("{line}");
        }
    }
    Ok(())
}
