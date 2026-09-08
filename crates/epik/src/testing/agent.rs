//! The scripted Agent: a small deterministic child, for tests.
//!
//! An Agent, not a mock of one: a real process through the real seam,
//! running by the time a constructor returns. Each constructor is one
//! behavior a test wants to observe. The child is always `sh -c
//! "<inline script>"` — inline only, never a written script file: this
//! codebase has been bitten by the ETXTBSY fork/exec race on freshly
//! written scripts, and inline sidesteps it entirely.

use crate::agent::Agent;
use crate::keystore::Secret;

/// Runs `script` under `sh -c`, from `/`, with nothing added to its
/// environment.
#[must_use]
pub fn shell(script: &str) -> Agent {
    sh(script, None)
}

/// Emits `line 1` through `line N` on stdout, then exits 0.
#[must_use]
pub fn counting(lines: usize) -> Agent {
    shell(&format!(
        "i=1; while [ \"$i\" -le {lines} ]; do echo \"line $i\"; i=$((i+1)); done"
    ))
}

/// Says `it all went wrong` on stderr and exits 3.
#[must_use]
pub fn failing() -> Agent {
    shell("echo 'it all went wrong' >&2; exit 3")
}

/// Echoes the environment variable `name`, whose value rides in as a
/// [`Secret`] — the deliberate exposure the env test observes.
#[must_use]
pub fn env_echo(name: &str, value: Secret) -> Agent {
    sh(
        &format!("printf '%s\\n' \"${name}\""),
        Some((name.to_owned(), value)),
    )
}

/// Hangs — a long sleep. The drop test's subject; the odd duration is
/// a marker the test can scan a process listing for.
#[must_use]
pub fn hanging() -> Agent {
    shell("sleep 6371")
}

/// `script` under `sh -c`, from `/`, with `env` in its environment.
fn sh(script: &str, env: Option<(String, Secret)>) -> Agent {
    Agent::new(
        vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        "/",
        env,
        None,
    )
    .expect("sh starts")
}
