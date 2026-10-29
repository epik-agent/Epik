//! What Epik needs on the machine, and whether it is in place.
//!
//! Each [`Item`] is a tool or a sign-in, with a check that runs commands
//! through a [`Runner`], so tests answer with recorded output. The setup
//! checklist shows every item's state, and a build is refused up front
//! when one it needs isn't done.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The oldest Node major version Epik runs on.
pub const NODE_MAJOR: u64 = 26;

/// What one command printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a program with arguments. A program that isn't installed is
/// [`io::ErrorKind::NotFound`].
pub trait Runner: Send + Sync {
    fn run(&self, program: &Path, args: &[&str]) -> io::Result<Output>;
}

/// Runs programs on this machine.
pub struct System;

impl Runner for System {
    fn run(&self, program: &Path, args: &[&str]) -> io::Result<Output> {
        let output = Command::new(program).args(args).output()?;
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Where an item stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Done,
    Missing,
    Outdated,
}

/// One thing Epik needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Item {
    #[serde(rename = "gh")]
    Gh,
    #[serde(rename = "gh-auth")]
    GhAuth,
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "claude-auth")]
    ClaudeAuth,
    #[serde(rename = "node")]
    Node,
    #[serde(rename = "plugin")]
    Plugin,
}

impl Item {
    /// Every item, in the checklist's order.
    pub const ALL: [Item; 6] = [
        Item::Gh,
        Item::GhAuth,
        Item::Claude,
        Item::ClaudeAuth,
        Item::Node,
        Item::Plugin,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Item::Gh => "gh",
            Item::GhAuth => "gh-auth",
            Item::Claude => "claude",
            Item::ClaudeAuth => "claude-auth",
            Item::Node => "node",
            Item::Plugin => "plugin",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Item::Gh => "GitHub CLI",
            Item::GhAuth => "GitHub sign-in",
            Item::Claude => "Claude Code",
            Item::ClaudeAuth => "Claude sign-in",
            Item::Node => "Node",
            Item::Plugin => "The Epik plugin",
        }
    }

    /// Whether a build needs it. A build loads the plugin from the
    /// directory it is given, so it doesn't need the plugin installed.
    pub fn needed_to_build(self) -> bool {
        self != Item::Plugin
    }
}

/// An item's state, with a line saying more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub item: Item,
    pub title: String,
    pub state: State,
    pub detail: String,
}

impl Check {
    fn new(item: Item, state: State, detail: impl Into<String>) -> Self {
        Self {
            item,
            title: item.title().to_owned(),
            state,
            detail: detail.into(),
        }
    }
}

/// The directory of Epik's private tools, `<root>/tools`.
pub fn tools_dir(root: &Path) -> PathBuf {
    root.join("tools")
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// The private copy of `gh`, `<root>/tools/gh/bin/gh`.
pub fn private_gh(root: &Path) -> PathBuf {
    tools_dir(root).join("gh").join("bin").join(exe("gh"))
}

/// The directory of the private copy of Node's programs: `bin` on macOS
/// and Linux, the copy itself on Windows.
pub fn private_node_bin(root: &Path) -> PathBuf {
    let node = tools_dir(root).join("node");
    if cfg!(windows) {
        node
    } else {
        node.join("bin")
    }
}

/// The private copy of Node.
pub fn private_node(root: &Path) -> PathBuf {
    private_node_bin(root).join(exe("node"))
}

/// The private copy of `program` if Epik installed one, else `program` as
/// found on the `PATH`.
fn located(private: PathBuf, program: &str) -> PathBuf {
    if private.is_file() {
        private
    } else {
        PathBuf::from(program)
    }
}

/// The `gh` Epik runs.
pub fn gh(root: &Path) -> PathBuf {
    located(private_gh(root), "gh")
}

/// The `node` Epik runs.
pub fn node(root: &Path) -> PathBuf {
    located(private_node(root), "node")
}

/// The `claude` Epik runs: where Anthropic's installer puts it,
/// `~/.local/bin`, which an app opened from the desktop may not have on
/// its `PATH`, else the `PATH`'s.
pub fn claude(root: &Path) -> PathBuf {
    let home = root.parent().unwrap_or(root);
    located(
        home.join(".local").join("bin").join(exe("claude")),
        "claude",
    )
}

/// Every item's state under `root`, normally `~/.epik`.
pub fn check_all(root: &Path, runner: &dyn Runner) -> Vec<Check> {
    Item::ALL
        .iter()
        .map(|&item| check(root, runner, item))
        .collect()
}

/// One item's state.
pub fn check(root: &Path, runner: &dyn Runner, item: Item) -> Check {
    match item {
        Item::Gh => version(runner, item, &gh(root), &["--version"], |out| {
            // "gh version 2.63.0 (2024-11-27)"
            out.split_whitespace().nth(2).map(|v| format!("gh {v}"))
        }),
        Item::GhAuth => gh_auth(root, runner),
        Item::Claude => version(runner, item, &claude(root), &["--version"], |out| {
            // "2.1.312 (Claude Code)"
            out.split_whitespace()
                .next()
                .map(|v| format!("Claude Code {v}"))
        }),
        Item::ClaudeAuth => claude_auth(root, runner),
        Item::Node => node_check(root, runner),
        Item::Plugin => plugin(root, runner),
    }
}

/// An item that is done when `program args` succeeds, saying the version
/// `describe` finds in its output.
fn version(
    runner: &dyn Runner,
    item: Item,
    program: &Path,
    args: &[&str],
    describe: impl Fn(&str) -> Option<String>,
) -> Check {
    match runner.run(program, args) {
        Ok(out) if out.success => {
            let detail = describe(out.stdout.trim()).unwrap_or_else(|| "found".to_owned());
            Check::new(item, State::Done, detail)
        }
        Ok(out) => Check::new(
            item,
            State::Missing,
            first_line(&out.stderr, "it doesn't run"),
        ),
        Err(_) => Check::new(item, State::Missing, "not installed"),
    }
}

fn first_line(text: &str, otherwise: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(otherwise)
        .to_owned()
}

fn gh_auth(root: &Path, runner: &dyn Runner) -> Check {
    let item = Item::GhAuth;
    let status = match runner.run(&gh(root), &["auth", "status"]) {
        Ok(status) => status,
        Err(_) => return Check::new(item, State::Missing, "gh isn't installed"),
    };
    if !status.success {
        return Check::new(item, State::Missing, "not signed in");
    }
    let text = format!("{}\n{}", status.stdout, status.stderr);
    let scopes = text
        .lines()
        .find(|line| line.contains("Token scopes:"))
        .unwrap_or_default();
    if !scopes.contains("'project'") {
        return Check::new(item, State::Missing, "signed in without the project scope");
    }
    let helper = runner.run(
        Path::new("git"),
        &[
            "config",
            "--global",
            "--get-all",
            "credential.https://github.com.helper",
        ],
    );
    match helper {
        Ok(out) if out.success && out.stdout.contains("auth git-credential") => {
            Check::new(item, State::Done, "signed in, with git using gh")
        }
        _ => Check::new(item, State::Missing, "git isn't set to use gh to push"),
    }
}

fn claude_auth(root: &Path, runner: &dyn Runner) -> Check {
    let item = Item::ClaudeAuth;
    let out = match runner.run(&claude(root), &["auth", "status"]) {
        Ok(out) => out,
        Err(_) => return Check::new(item, State::Missing, "Claude Code isn't installed"),
    };
    let status: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    if status["loggedIn"] == true {
        let how = status["authMethod"].as_str().unwrap_or("an account");
        Check::new(item, State::Done, format!("signed in with {how}"))
    } else {
        Check::new(item, State::Missing, "not signed in")
    }
}

/// Node's major version from `node --version`'s `v26.1.0`.
fn node_major(version: &str) -> Option<u64> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn node_check(root: &Path, runner: &dyn Runner) -> Check {
    let item = Item::Node;
    match runner.run(&node(root), &["--version"]) {
        Ok(out) if out.success => {
            let version = out.stdout.trim().trim_start_matches('v').to_owned();
            match node_major(&version) {
                Some(major) if major >= NODE_MAJOR => {
                    Check::new(item, State::Done, format!("Node {version}"))
                }
                _ => Check::new(
                    item,
                    State::Outdated,
                    format!("Node {version}; Epik needs {NODE_MAJOR} or later"),
                ),
            }
        }
        _ => Check::new(item, State::Missing, "not installed"),
    }
}

fn plugin(root: &Path, runner: &dyn Runner) -> Check {
    let item = Item::Plugin;
    let out = match runner.run(&claude(root), &["plugin", "list", "--json"]) {
        Ok(out) if out.success => out,
        Ok(_) => {
            return Check::new(
                item,
                State::Missing,
                "Claude Code couldn't list its plugins",
            );
        }
        Err(_) => return Check::new(item, State::Missing, "Claude Code isn't installed"),
    };
    let plugins: Vec<Value> = serde_json::from_str(&out.stdout).unwrap_or_default();
    match plugins.iter().find(|plugin| plugin["id"] == "epik@epik") {
        Some(plugin) if plugin["enabled"] == true => Check::new(item, State::Done, "installed"),
        Some(_) => Check::new(item, State::Missing, "installed but turned off"),
        None => Check::new(item, State::Missing, "not installed"),
    }
}

/// Why a build can't start, if an item it needs isn't done: naming them,
/// and saying where to fix it.
pub fn refusal(checks: &[Check]) -> Option<String> {
    let missing: Vec<&str> = checks
        .iter()
        .filter(|check| check.item.needed_to_build() && check.state != State::Done)
        .map(|check| check.title.as_str())
        .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "Epik's setup is incomplete: {}. Choose Check setup in the Epik menu.",
            missing.join(", ")
        )
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A runner answering each command, written `program args…`, with a
    /// recorded output; a command it doesn't know isn't installed.
    #[derive(Default, Clone)]
    pub(crate) struct Recorded(BTreeMap<String, Output>);

    impl Recorded {
        pub(crate) fn answer(mut self, command: &str, success: bool, stdout: &str) -> Self {
            self.0.insert(
                command.to_owned(),
                Output {
                    success,
                    stdout: stdout.to_owned(),
                    stderr: String::new(),
                },
            );
            self
        }

        /// Everything set up, as on a machine where it all works.
        pub(crate) fn complete() -> Self {
            Self::default()
                .answer("gh --version", true, "gh version 2.102.0 (2026-09-30)\n")
                .answer("gh auth status", true, GH_SIGNED_IN)
                .answer(
                    "git config --global --get-all credential.https://github.com.helper",
                    true,
                    "!/opt/homebrew/bin/gh auth git-credential\n",
                )
                .answer("claude --version", true, "2.1.312 (Claude Code)\n")
                .answer("claude auth status", true, CLAUDE_SIGNED_IN)
                .answer("node --version", true, "v26.10.0\n")
                .answer("claude plugin list --json", true, PLUGINS)
        }
    }

    impl Runner for Recorded {
        fn run(&self, program: &Path, args: &[&str]) -> io::Result<Output> {
            let name = program.file_name().unwrap_or_default().to_string_lossy();
            let command = std::iter::once(name.as_ref())
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.0
                .get(&command)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, command))
        }
    }

    const GH_SIGNED_IN: &str = "github.com\n  \u{2713} Logged in to github.com account someone (keyring)\n  - Active account: true\n  - Git operations protocol: ssh\n  - Token: gho_************************************\n  - Token scopes: 'gist', 'project', 'repo', 'workflow'\n";
    const CLAUDE_SIGNED_IN: &str =
        r#"{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"}"#;
    const PLUGINS: &str = r#"[{"id": "codex@openai-codex", "enabled": true}, {"id": "epik@epik", "version": "b7d47eea19b5", "scope": "user", "enabled": true}]"#;

    fn state(runner: &Recorded, item: Item) -> (State, String) {
        let root = tempfile::tempdir().unwrap();
        let check = check(&root.path().join(".epik"), runner, item);
        (check.state, check.detail)
    }

    #[test]
    fn on_a_complete_machine_every_item_is_done() {
        let root = tempfile::tempdir().unwrap();
        let checks = check_all(&root.path().join(".epik"), &Recorded::complete());

        let details: Vec<(&str, State, &str)> = checks
            .iter()
            .map(|c| (c.item.id(), c.state, c.detail.as_str()))
            .collect();
        assert_eq!(
            details,
            [
                ("gh", State::Done, "gh 2.102.0"),
                ("gh-auth", State::Done, "signed in, with git using gh"),
                ("claude", State::Done, "Claude Code 2.1.312"),
                ("claude-auth", State::Done, "signed in with claude.ai"),
                ("node", State::Done, "Node 26.10.0"),
                ("plugin", State::Done, "installed"),
            ]
        );
        assert_eq!(refusal(&checks), None);
    }

    #[test]
    fn on_an_empty_machine_every_item_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let checks = check_all(&root.path().join(".epik"), &Recorded::default());

        assert!(
            checks.iter().all(|c| c.state == State::Missing),
            "{checks:?}"
        );
        assert_eq!(
            refusal(&checks).unwrap(),
            "Epik's setup is incomplete: GitHub CLI, GitHub sign-in, Claude Code, \
             Claude sign-in, Node. Choose Check setup in the Epik menu."
        );
    }

    #[test]
    fn gh_signed_out_or_without_the_project_scope_or_git_is_not_signed_in() {
        let signed_out = Recorded::complete().answer("gh auth status", false, "");
        assert_eq!(
            state(&signed_out, Item::GhAuth),
            (State::Missing, "not signed in".into())
        );

        let no_project = GH_SIGNED_IN.replace("'project', ", "");
        let no_project = Recorded::complete().answer("gh auth status", true, &no_project);
        assert_eq!(
            state(&no_project, Item::GhAuth),
            (State::Missing, "signed in without the project scope".into())
        );

        let no_git = Recorded::complete().answer(
            "git config --global --get-all credential.https://github.com.helper",
            false,
            "",
        );
        assert_eq!(
            state(&no_git, Item::GhAuth),
            (State::Missing, "git isn't set to use gh to push".into())
        );
    }

    #[test]
    fn claude_signed_out_is_missing() {
        let out =
            Recorded::complete().answer("claude auth status", false, r#"{"loggedIn": false}"#);
        assert_eq!(
            state(&out, Item::ClaudeAuth),
            (State::Missing, "not signed in".into())
        );
    }

    #[test]
    fn node_older_than_26_is_outdated() {
        let old = Recorded::complete().answer("node --version", true, "v22.11.0\n");
        assert_eq!(
            state(&old, Item::Node),
            (
                State::Outdated,
                "Node 22.11.0; Epik needs 26 or later".into()
            )
        );
        let checks = check_all(Path::new("/nowhere/.epik"), &old);
        assert_eq!(
            refusal(&checks).unwrap(),
            "Epik's setup is incomplete: Node. Choose Check setup in the Epik menu."
        );
    }

    #[test]
    fn the_plugin_is_missing_when_not_installed_or_turned_off() {
        let none = Recorded::complete().answer("claude plugin list --json", true, "[]");
        assert_eq!(
            state(&none, Item::Plugin),
            (State::Missing, "not installed".into())
        );
        let off = PLUGINS.replace(r#""scope": "user", "enabled": true"#, r#""enabled": false"#);
        let off = Recorded::complete().answer("claude plugin list --json", true, &off);
        assert_eq!(
            state(&off, Item::Plugin),
            (State::Missing, "installed but turned off".into())
        );
    }

    #[test]
    fn the_plugin_isnt_needed_to_build() {
        let root = tempfile::tempdir().unwrap();
        let no_plugin = Recorded::complete().answer("claude plugin list --json", true, "[]");

        assert_eq!(refusal(&check_all(root.path(), &no_plugin)), None);
    }

    #[test]
    fn a_private_copy_is_found_before_the_path() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(gh(root.path()), PathBuf::from("gh"));
        assert_eq!(node(root.path()), PathBuf::from("node"));

        for private in [private_gh(root.path()), private_node(root.path())] {
            std::fs::create_dir_all(private.parent().unwrap()).unwrap();
            std::fs::write(&private, "").unwrap();
        }

        assert_eq!(gh(root.path()), private_gh(root.path()));
        assert_eq!(node(root.path()), private_node(root.path()));
    }

    #[test]
    fn items_serialize_by_id() {
        let check = Check::new(Item::GhAuth, State::Missing, "not signed in");
        assert_eq!(
            serde_json::to_value(&check).unwrap(),
            serde_json::json!({
                "item": "gh-auth",
                "title": "GitHub sign-in",
                "state": "missing",
                "detail": "not signed in",
            })
        );
    }
}
