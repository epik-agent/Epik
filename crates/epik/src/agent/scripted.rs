//! The scripted Agent: a small deterministic child, for tests.
//!
//! An Agent, not a mock of one: it runs through the real runner like any
//! other. Each constructor is one behavior a test wants to observe. The
//! child is always `sh -c "<inline script>"` — inline only, never a
//! written script file: this codebase has been bitten by the ETXTBSY
//! fork/exec race on freshly written scripts, and inline sidesteps it
//! entirely.

use super::{Agent, Task};
use crate::keystore::Secret;

/// A deterministic Agent, one behavior per constructor.
pub struct Scripted(Task);

impl Scripted {
    fn shell(script: String) -> Self {
        Self(Task {
            argv: vec!["sh".to_owned(), "-c".to_owned(), script],
            env: Vec::new(),
            cwd: "/".to_owned(),
            stdin: None,
        })
    }

    /// Emits `line 1` through `line N` on stdout, then exits 0.
    #[must_use]
    pub fn counting(lines: usize) -> Self {
        Self::shell(format!(
            "i=1; while [ \"$i\" -le {lines} ]; do echo \"line $i\"; i=$((i+1)); done"
        ))
    }

    /// Says `it all went wrong` on stderr and exits 3.
    #[must_use]
    pub fn failing() -> Self {
        Self::shell("echo 'it all went wrong' >&2; exit 3".to_owned())
    }

    /// Echoes the environment variable `name`, whose value rides in as a
    /// [`Secret`] — the deliberate exposure the env test observes.
    #[must_use]
    pub fn env_echo(name: &str, value: Secret) -> Self {
        let mut agent = Self::shell(format!("printf '%s\\n' \"${name}\""));
        agent.0.env.push((name.to_owned(), value));
        agent
    }

    /// Echoes its stdin payload back on stdout.
    #[must_use]
    pub fn stdin_echo(payload: &str) -> Self {
        let mut agent = Self::shell("cat".to_owned());
        agent.0.stdin = Some(payload.to_owned());
        agent
    }

    /// Hangs — a long sleep. The kill test's subject; the odd duration
    /// is a marker the test can scan a process listing for.
    #[must_use]
    pub fn hanging() -> Self {
        Self::shell("sleep 6371".to_owned())
    }
}

impl Agent for Scripted {
    fn task(&self) -> Task {
        self.0.clone()
    }
}
