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

/// The latest release of a tool Epik keeps a private copy of.
pub trait Latest: Send + Sync {
    /// The latest version of `tool`, `gh` or `node`, if known.
    fn version(&self, tool: Item) -> Option<String>;
}

/// Knows no releases, so no private copy is ever outdated.
pub struct Unknown;

impl Latest for Unknown {
    fn version(&self, _tool: Item) -> Option<String> {
        None
    }
}

/// Whether dotted version `a` is older than `b`.
pub fn older(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parts(a) < parts(b)
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
    /// Whether Epik can use it as it is. A private copy behind the latest
    /// release is outdated but usable; Node older than Epik needs is not.
    pub usable: bool,
}

impl Check {
    fn new(item: Item, state: State, detail: impl Into<String>) -> Self {
        Self {
            item,
            title: item.title().to_owned(),
            state,
            detail: detail.into(),
            usable: state == State::Done,
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

/// Every item's state under `root`, normally `~/.epik`. A private copy
/// older than `latest`'s release is outdated.
pub fn check_all(root: &Path, runner: &dyn Runner, latest: &dyn Latest) -> Vec<Check> {
    Item::ALL
        .iter()
        .map(|&item| check(root, runner, latest, item))
        .collect()
}

/// One item's state.
pub fn check(root: &Path, runner: &dyn Runner, latest: &dyn Latest, item: Item) -> Check {
    let check = check_found(root, runner, item);
    let private = match item {
        Item::Gh => private_gh(root),
        Item::Node => private_node(root),
        _ => return check,
    };
    if check.state != State::Done || !private.is_file() {
        return check;
    }
    let found = check
        .detail
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    match latest.version(item) {
        Some(newest) if older(&found, &newest) => Check {
            usable: true,
            ..Check::new(
                item,
                State::Outdated,
                format!("{}; {newest} is out", check.detail),
            )
        },
        _ => check,
    }
}

fn check_found(root: &Path, runner: &dyn Runner, item: Item) -> Check {
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

/// `PATH` with Epik's private tools first, then the places installers put
/// programs that an app opened from the desktop may not have on its
/// `PATH`, then `current`. Programs Epik runs, and the agents it starts,
/// find the same tools the checks do.
pub fn search_path(root: &Path, current: Option<std::ffi::OsString>) -> std::ffi::OsString {
    let home = root.parent().unwrap_or(root);
    let mut dirs = vec![
        tools_dir(root).join("gh").join("bin"),
        private_node_bin(root),
        home.join(".local").join("bin"),
    ];
    if cfg!(unix) {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    }
    let existing: Vec<PathBuf> = current
        .as_deref()
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    dirs.retain(|dir| !existing.contains(dir));
    dirs.extend(existing);
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Why a build can't start, if an item it needs isn't done: naming them,
/// and saying where to fix it.
pub fn refusal(checks: &[Check]) -> Option<String> {
    let missing: Vec<&str> = checks
        .iter()
        .filter(|check| check.item.needed_to_build() && !check.usable)
        .map(|check| check.title.as_str())
        .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "Epik's setup is incomplete: {}. Choose Check setup in the Epik menu.",
            missing.join(", ")
        )
    })
}

/// What pressing an item's button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Install Epik's private copy of `gh` or Node.
    Install(Item),
    /// Run these commands in order, each a program and its arguments,
    /// stopping at the first that fails.
    Run(Vec<Vec<String>>),
}

/// The one button an item that isn't done offers: what it says, and what
/// it does. Nothing happens until the user presses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub label: String,
    pub action: Action,
}

fn command(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// The step for `check`, or none when it's done.
pub fn step(check: &Check) -> Option<Step> {
    if check.state == State::Done {
        return None;
    }
    let (label, action) = match check.item {
        Item::Gh | Item::Node => {
            let label = if check.state == State::Outdated {
                "Update the private copy"
            } else {
                "Install a private copy"
            };
            (label.to_owned(), Action::Install(check.item))
        }
        Item::GhAuth if check.detail == "signed in without the project scope" => (
            "Add the project scope (opens the browser)".to_owned(),
            Action::Run(vec![
                command(&["gh", "auth", "refresh", "-h", "github.com", "-s", "project"]),
                command(&["gh", "auth", "setup-git"]),
            ]),
        ),
        Item::GhAuth if check.detail == "git isn't set to use gh to push" => (
            "Let git push with gh".to_owned(),
            Action::Run(vec![command(&["gh", "auth", "setup-git"])]),
        ),
        Item::GhAuth => (
            "Sign in to GitHub (opens the browser)".to_owned(),
            Action::Run(vec![
                command(&[
                    "gh",
                    "auth",
                    "login",
                    "--web",
                    "-h",
                    "github.com",
                    "-p",
                    "https",
                    "-s",
                    "project",
                ]),
                command(&["gh", "auth", "setup-git"]),
            ]),
        ),
        Item::Claude if cfg!(windows) => (
            "Run Anthropic's installer: irm https://claude.ai/install.ps1 | iex".to_owned(),
            Action::Run(vec![command(&[
                "powershell",
                "-NoProfile",
                "-Command",
                "irm https://claude.ai/install.ps1 | iex",
            ])]),
        ),
        Item::Claude => (
            "Run Anthropic's installer: curl -fsSL https://claude.ai/install.sh | bash".to_owned(),
            Action::Run(vec![command(&[
                "bash",
                "-c",
                "curl -fsSL https://claude.ai/install.sh | bash",
            ])]),
        ),
        Item::ClaudeAuth => (
            "Sign in to Claude (opens the browser)".to_owned(),
            Action::Run(vec![command(&["claude", "auth", "login"])]),
        ),
        Item::Plugin if check.detail == "installed but turned off" => (
            "Turn the plugin on".to_owned(),
            Action::Run(vec![command(&["claude", "plugin", "enable", "epik@epik"])]),
        ),
        Item::Plugin if check.state == State::Outdated => (
            "Update the plugin".to_owned(),
            Action::Run(vec![
                command(&["claude", "plugin", "marketplace", "update", "epik"]),
                command(&["claude", "plugin", "update", "epik@epik"]),
            ]),
        ),
        Item::Plugin => (
            "Install the plugin".to_owned(),
            Action::Run(vec![
                command(&["claude", "plugin", "marketplace", "add", "epik-agent/Epik"]),
                command(&[
                    "claude",
                    "plugin",
                    "install",
                    "epik@epik",
                    "--scope",
                    "user",
                ]),
            ]),
        ),
    };
    Some(Step { label, action })
}

/// The program to run for a step's command word: Epik's own `gh` or
/// `claude` where it would check them, else the word as found on the
/// `PATH`.
pub fn program(root: &Path, word: &str) -> PathBuf {
    match word {
        "gh" => gh(root),
        "claude" => claude(root),
        other => PathBuf::from(other),
    }
}

/// Does `step`, telling `on_line` each line of output as it comes, or
/// why it failed.
pub fn run_step(root: &Path, step: &Step, on_line: &(dyn Fn(String) + Sync)) -> Result<(), String> {
    match &step.action {
        Action::Install(item) => {
            let platform = crate::install::Platform::current()
                .ok_or("Epik installs no tools for this machine")?;
            let sources = crate::install::Sources::default();
            on_line(format!("Downloading {}…", item.title()));
            let version = match item {
                Item::Gh => crate::install::install_gh(root, &sources, platform)?,
                _ => crate::install::install_node(root, &sources, platform)?,
            };
            on_line(format!(
                "Installed {} {version}, checked against its published checksum.",
                item.title()
            ));
            Ok(())
        }
        Action::Run(commands) => {
            for words in commands {
                on_line(format!("$ {}", words.join(" ")));
                run_streaming(&program(root, &words[0]), &words[1..], on_line)?;
            }
            Ok(())
        }
    }
}

/// Runs `program` with `args`, its stdin closed so a prompt to press Enter
/// goes on by itself, telling `on_line` each line of its stdout and stderr.
fn run_streaming(
    program: &Path,
    args: &[String],
    on_line: &(dyn Fn(String) + Sync),
) -> Result<(), String> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let name = program.display().to_string();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("couldn't run {name}: {error}"))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                on_line(line);
            }
        });
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            on_line(line);
        }
    });
    let status = child
        .wait()
        .map_err(|error| format!("{name} didn't finish: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} {} failed ({status})", args.join(" ")))
    }
}

/// The file whose presence says setup was once completed, so the checklist
/// doesn't open by itself again while everything stays done.
pub fn done_file(root: &Path) -> PathBuf {
    root.join("setup-done")
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
            let name = program.file_stem().unwrap_or_default().to_string_lossy();
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
        let check = check(&root.path().join(".epik"), runner, &Unknown, item);
        (check.state, check.detail)
    }

    #[test]
    fn on_a_complete_machine_every_item_is_done() {
        let root = tempfile::tempdir().unwrap();
        let checks = check_all(&root.path().join(".epik"), &Recorded::complete(), &Unknown);

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
        let checks = check_all(&root.path().join(".epik"), &Recorded::default(), &Unknown);

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
        let checks = check_all(Path::new("/nowhere/.epik"), &old, &Unknown);
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

        assert_eq!(refusal(&check_all(root.path(), &no_plugin, &Unknown)), None);
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

    struct Newest;

    impl Latest for Newest {
        fn version(&self, tool: Item) -> Option<String> {
            Some(
                if tool == Item::Gh {
                    "2.200.0"
                } else {
                    "26.10.0"
                }
                .to_owned(),
            )
        }
    }

    #[test]
    fn a_private_copy_older_than_the_latest_release_is_outdated() {
        let root = tempfile::tempdir().unwrap();
        let runner = Recorded::complete();

        let path_gh = check(root.path(), &runner, &Newest, Item::Gh);
        assert_eq!(
            path_gh.state,
            State::Done,
            "only private copies are Epik's to update"
        );

        std::fs::create_dir_all(private_gh(root.path()).parent().unwrap()).unwrap();
        std::fs::write(private_gh(root.path()), "").unwrap();
        let private = check(root.path(), &runner, &Newest, Item::Gh);
        assert_eq!(
            (private.state, private.detail.as_str()),
            (State::Outdated, "gh 2.102.0; 2.200.0 is out")
        );
        assert!(private.usable);
        assert_eq!(refusal(&[private]), None, "a build can use it as it is");
        std::fs::create_dir_all(private_node(root.path()).parent().unwrap()).unwrap();
        std::fs::write(private_node(root.path()), "").unwrap();
        let node = check(root.path(), &runner, &Newest, Item::Node);
        assert_eq!(node.state, State::Done, "26.10.0 is the latest");
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(older("2.9.0", "2.10.0"));
        assert!(older("v26.1.0", "26.10.0"));
        assert!(!older("2.10.0", "2.9.0"));
        assert!(!older("2.10.0", "2.10.0"));
    }

    #[test]
    fn private_tools_come_first_on_the_path() {
        let root = Path::new("/home/me/.epik");
        let current = std::env::join_paths([
            PathBuf::from("/usr/bin"),
            PathBuf::from("/opt/homebrew/bin"),
        ])
        .unwrap();

        let path = search_path(root, Some(current));

        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(dirs[0], private_gh(root).parent().unwrap());
        assert_eq!(dirs[1], private_node_bin(root));
        assert_eq!(dirs[2], Path::new("/home/me/.local/bin"));
        assert!(dirs.ends_with(&[
            PathBuf::from("/usr/bin"),
            PathBuf::from("/opt/homebrew/bin")
        ]));
        assert_eq!(
            dirs.iter()
                .filter(|dir| dir.ends_with("homebrew/bin"))
                .count(),
            1
        );
    }

    fn step_of(item: Item, state: State, detail: &str) -> Option<Step> {
        step(&Check::new(item, state, detail))
    }

    #[test]
    fn a_done_item_offers_nothing() {
        for item in Item::ALL {
            assert_eq!(step_of(item, State::Done, "fine"), None);
        }
    }

    #[test]
    fn each_item_offers_the_one_step_that_does_it() {
        let label = |item, state, detail| step_of(item, state, detail).unwrap().label;
        assert_eq!(
            label(Item::Gh, State::Missing, "not installed"),
            "Install a private copy"
        );
        assert_eq!(
            label(Item::Node, State::Outdated, "Node 22"),
            "Update the private copy"
        );
        assert_eq!(
            step_of(Item::Node, State::Missing, "not installed")
                .unwrap()
                .action,
            Action::Install(Item::Node)
        );
        assert_eq!(
            step_of(Item::GhAuth, State::Missing, "not signed in").unwrap(),
            Step {
                label: "Sign in to GitHub (opens the browser)".to_owned(),
                action: Action::Run(vec![
                    command(&[
                        "gh",
                        "auth",
                        "login",
                        "--web",
                        "-h",
                        "github.com",
                        "-p",
                        "https",
                        "-s",
                        "project"
                    ]),
                    command(&["gh", "auth", "setup-git"]),
                ]),
            }
        );
        assert_eq!(
            step_of(
                Item::GhAuth,
                State::Missing,
                "signed in without the project scope"
            )
            .unwrap()
            .action,
            Action::Run(vec![
                command(&["gh", "auth", "refresh", "-h", "github.com", "-s", "project"]),
                command(&["gh", "auth", "setup-git"]),
            ])
        );
        assert_eq!(
            label(
                Item::GhAuth,
                State::Missing,
                "git isn't set to use gh to push"
            ),
            "Let git push with gh"
        );
        assert!(
            label(Item::Claude, State::Missing, "not installed")
                .starts_with("Run Anthropic's installer: ")
        );
        assert_eq!(
            step_of(Item::ClaudeAuth, State::Missing, "not signed in")
                .unwrap()
                .action,
            Action::Run(vec![command(&["claude", "auth", "login"])])
        );
        assert_eq!(
            step_of(Item::Plugin, State::Missing, "not installed")
                .unwrap()
                .action,
            Action::Run(vec![
                command(&["claude", "plugin", "marketplace", "add", "epik-agent/Epik"]),
                command(&[
                    "claude",
                    "plugin",
                    "install",
                    "epik@epik",
                    "--scope",
                    "user"
                ]),
            ])
        );
        assert_eq!(
            label(Item::Plugin, State::Missing, "installed but turned off"),
            "Turn the plugin on"
        );
        assert_eq!(
            label(Item::Plugin, State::Outdated, "behind"),
            "Update the plugin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_steps_commands_run_in_order_with_their_output_streamed() {
        let root = tempfile::tempdir().unwrap();
        let lines = std::sync::Mutex::new(Vec::new());
        let step = Step {
            label: "Do it".to_owned(),
            action: Action::Run(vec![
                command(&["sh", "-c", "echo one"]),
                command(&["sh", "-c", "echo two >&2"]),
            ]),
        };

        run_step(root.path(), &step, &|line| lines.lock().unwrap().push(line)).unwrap();

        assert_eq!(
            *lines.lock().unwrap(),
            ["$ sh -c echo one", "one", "$ sh -c echo two >&2", "two"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_stops_the_step() {
        let root = tempfile::tempdir().unwrap();
        let lines = std::sync::Mutex::new(Vec::new());
        let step = Step {
            label: "Do it".to_owned(),
            action: Action::Run(vec![
                command(&["sh", "-c", "exit 3"]),
                command(&["sh", "-c", "echo never"]),
            ]),
        };

        let error =
            run_step(root.path(), &step, &|line| lines.lock().unwrap().push(line)).unwrap_err();

        assert!(error.starts_with("sh -c exit 3 failed"), "{error}");
        assert_eq!(*lines.lock().unwrap(), ["$ sh -c exit 3"]);
        let missing = Step {
            label: "Do it".to_owned(),
            action: Action::Run(vec![command(&["no-such-program-epik"])]),
        };
        let error = run_step(root.path(), &missing, &|_| {}).unwrap_err();
        assert!(
            error.starts_with("couldn't run no-such-program-epik"),
            "{error}"
        );
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
                "usable": false,
            })
        );
    }
}
