//! Epik's own git: the user's `git` binary, run as argv.
//!
//! Every invocation is argv straight into an [`Agent`] — never a shell,
//! never a string spliced into a command line — under a deadline, with
//! no terminal to prompt on. [`execute`] settles the outcome into the
//! `{ ok, output }` shape the model's git tools answer with, and
//! [`plumbing`] is the same run for callers inside the crate, who want
//! git's answer as a value and its refusal as words. The persona
//! identity every commit Epik itself makes is stated here, once.

use std::time::Duration;

use serde_json::{Value, json};

use crate::agent::Agent;
use crate::keystore::Secret;

/// How long one git invocation may take before it is killed and reported.
const TIMEOUT: Duration = Duration::from_secs(60);

/// The Epik persona's git identity: what its own commits are authored as
/// — the root commit `git_init` makes, and everything a build agent
/// commits in a provisioned worktree.
pub(crate) const PERSONA_NAME: &str = "Epik";
pub(crate) const PERSONA_EMAIL: &str = "epik@epik-agent.dev";

/// Runs git with `args` as argv — no shell anywhere — and settles the
/// outcome into the one shape the model's git tools answer with:
/// `{ ok, output }`, where a nonzero exit is `ok: false` and `output`
/// carries git's own words, stdout and stderr both.
pub(crate) fn execute(args: &[&str]) -> Result<Value, String> {
    execute_within(args, &[], TIMEOUT)
}

/// [`execute`] against a stated deadline — which is how the tests
/// exercise the kill without sitting through the real one — and with
/// extra environment, which is how a push's credentials ride askpass
/// rather than argv.
fn execute_within(
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
) -> Result<Value, String> {
    let argv = std::iter::once("git")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect();
    // No terminal is attached, so a network verb that wants credentials
    // must fail in words rather than wait for a prompt nobody can see.
    // The user's helpers and SSH config still apply.
    let env = envs
        .iter()
        .chain(&[("GIT_TERMINAL_PROMPT", "0")])
        .map(|(name, value)| ((*name).to_owned(), Secret::from(*value)));
    // From wherever Epik is: every call names its repository with `-C`.
    let finished = Agent::new(argv, ".", env, Some(timeout))
        .and_then(Agent::finish)
        .map_err(|error| format!("{error:#}"))?;
    Ok(json!({ "ok": finished.exit.success(), "output": finished.output }))
}

/// [`execute`] for callers inside the crate that want git's answer, not
/// a tool result: the combined output on success, git's own words as the
/// Err on a nonzero exit.
pub(crate) fn plumbing(args: &[&str]) -> Result<String, String> {
    plumbing_with(args, &[])
}

/// The commit `base` names in `repository` — a branch, a tag, a sha —
/// through `rev-parse --verify`, so the words name the base that did
/// not.
pub(crate) fn base_commit(repository: &str, base: &str) -> Result<String, String> {
    plumbing(&[
        "-C",
        repository,
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{base}^{{commit}}"),
    ])
    .map(|sha| sha.trim().to_owned())
    .map_err(|words| format!("the base {base:?} does not name a commit: {words}"))
}

/// [`plumbing`] with extra environment, for the one caller whose git
/// must be told things argv may not carry: the push, whose credentials
/// answer askpass through variables that die with the process.
pub(crate) fn plumbing_with(args: &[&str], envs: &[(&str, &str)]) -> Result<String, String> {
    let value = execute_within(args, envs, TIMEOUT)?;
    let output = value["output"].as_str().unwrap_or_default().to_owned();
    if value["ok"].as_bool().unwrap_or(false) {
        Ok(output)
    } else if output.trim().is_empty() {
        Err(format!("git {} failed without saying why", args.join(" ")))
    } else {
        Err(output.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{Scratch, seeded};

    /// The deadline, exercised on a short fuse rather than the real 60
    /// seconds: a fetch against a loopback listener that accepts and then
    /// says nothing blocks git forever, so the Agent has to kill it and
    /// say so.
    #[test]
    fn the_deadline_kills_a_git_that_will_not_finish() {
        let scratch = Scratch::new("deadline");
        let (work, _) = seeded(&scratch);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("git://{}/hung", listener.local_addr().unwrap());
        // Accepts and then holds the socket, answering nothing, until the
        // test process ends; detached, so the test cannot hang on it.
        std::thread::spawn(move || {
            let _held = listener.accept();
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        });

        let error = execute_within(
            &["-C", &work, "fetch", &url],
            &[],
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(error.contains("killed"), "{error}");
    }
}
