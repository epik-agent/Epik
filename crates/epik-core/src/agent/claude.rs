//! Claude Code, the first real engine: a [`Process`] whose program is the
//! `claude` CLI in headless print mode.
//!
//! [`ClaudeCode`] is data — a binary path, a working directory, a
//! prompt, and optionally a model and an API key — and [`start`] is the
//! one place it becomes a command line. That line pins Epik's settled
//! isolation posture: the agent runs with Epik's configuration
//! (`--settings '{}'`, `--strict-mcp-config`), never the machine-wide
//! Claude Code setup, and with `--dangerously-skip-permissions` —
//! deliberate autonomy, because output only flows *out* of an agent;
//! there is no channel to answer a permission prompt, so the agent must
//! never ask one. Its containment is the working directory and the
//! environment it is handed. The prompt is the last argument: the CLI
//! takes it positionally, and a Process has no stdin.
//!
//! What the CLI says on stdout is its stream-json protocol, modelled
//! and read in [`protocol`].
//!
//! [`start`]: Agent::start

#[cfg(feature = "native")]
mod protocol;

#[cfg(feature = "native")]
use super::{Agent, Update};
#[cfg(feature = "native")]
use crate::keystore::Secret;
#[cfg(feature = "native")]
use crate::process::Process;
#[cfg(feature = "native")]
use std::path::PathBuf;

/// The `claude` CLI as an [`Agent`]. Data only: the caller supplies the
/// binary's absolute path — no PATH divination — and resolves any API
/// key itself; this type never touches the keystore. With no key, the
/// CLI's own logged-in auth applies; both are legitimate.
#[cfg(feature = "native")]
#[derive(Clone, Debug)]
pub struct ClaudeCode {
    pub binary: PathBuf,
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    pub api_key: Option<Secret>,
}

#[cfg(feature = "native")]
impl Agent for ClaudeCode {
    /// The CLI in its working directory, the key in its environment
    /// when there is one.
    fn start(self) -> anyhow::Result<Process> {
        let arguments = self.arguments();
        let env = self
            .api_key
            .map(|key| ("ANTHROPIC_API_KEY".to_owned(), key));
        Process::new(self.binary, arguments, self.cwd, env, None)
    }

    fn interpret(line: &str) -> Vec<Update> {
        protocol::interpret(line)
    }
}

#[cfg(feature = "native")]
impl ClaudeCode {
    /// The arguments after the binary: the settled posture, the model
    /// when one is named, and the prompt last.
    fn arguments(&self) -> Vec<String> {
        let mut argv = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            // Stream output requires it.
            "--verbose".to_owned(),
        ];
        if let Some(model) = &self.model {
            argv.push("--model".to_owned());
            argv.push(model.clone());
        }
        argv.extend(
            [
                // Epik's isolation posture: the agent runs with Epik's
                // configuration, not the user's machine-wide setup...
                "--settings",
                "{}",
                "--strict-mcp-config",
                // ...and with deliberate autonomy: no channel exists to
                // answer a prompt, so none may be asked.
                "--dangerously-skip-permissions",
            ]
            .map(str::to_owned),
        );
        // The prompt is positional, and last: any length, any number
        // of lines, and never mistaken for a flag.
        argv.push(self.prompt.clone());
        argv
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    const ANTHROPIC_MODEL: &str = "claude-sonnet-5";

    fn agent() -> ClaudeCode {
        ClaudeCode {
            binary: "/nonexistent/claude".into(),
            cwd: "/work/repo".into(),
            prompt: "create a file\nnamed hello.txt".to_owned(),
            model: Some(ANTHROPIC_MODEL.to_owned()),
            api_key: Some(Secret::from("sk-ant-hush-hush")),
        }
    }

    /// The isolation and skip-permissions flags are load-bearing;
    /// the whole argument list is pinned, the prompt last and the key
    /// nowhere in it.
    #[test]
    fn the_arguments_are_exactly_the_settled_posture() {
        assert_eq!(
            agent().arguments(),
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                ANTHROPIC_MODEL,
                "--settings",
                "{}",
                "--strict-mcp-config",
                "--dangerously-skip-permissions",
                "create a file\nnamed hello.txt",
            ]
        );

        let mut without_model = agent();
        without_model.model = None;
        assert_eq!(
            without_model.arguments(),
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--settings",
                "{}",
                "--strict-mcp-config",
                "--dangerously-skip-permissions",
                "create a file\nnamed hello.txt",
            ]
        );
    }

    #[test]
    fn no_debug_output_carries_the_keys_bytes() {
        let agent = agent();
        let debugged = format!("{agent:?}");
        assert!(!debugged.contains("sk-ant-hush-hush"), "{debugged}");
        assert!(debugged.contains("api_key"), "the field is there, redacted");
    }

    /// The binary is at a path no machine has, so the spawn fails —
    /// after the recipe was read in full, which is what the
    /// refusal's words show.
    #[test]
    fn a_missing_binary_refuses_in_words() {
        let error = agent().start().err().expect("no such binary");
        assert!(
            format!("{error:#}").contains("could not start /nonexistent/claude"),
            "{error:#}"
        );
    }
}
