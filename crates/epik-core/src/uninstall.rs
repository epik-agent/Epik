//! Removing what Epik put on the machine.
//!
//! Uninstalling takes out the Epik plugin and its marketplace from Claude
//! Code, then all of `~/.epik`: the private tools, the app's link to
//! itself, the build clones and the run history. It leaves alone what the
//! user may also use without Epik: Claude Code and its sign-in, `gh` on
//! the `PATH`, the `gh` sign-in, and git's use of `gh` as its credential
//! helper. The app doesn't delete itself; the user does that last.

use std::fs;
use std::io;
use std::path::Path;

use crate::cleanup::builds_dir;
use crate::record::runs_dir;
use crate::setup::{self, Runner, tools_dir};

/// One thing uninstalling removes, in the order it removes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `claude plugin uninstall epik@epik`.
    Plugin,
    /// `claude plugin marketplace remove epik`.
    Marketplace,
    /// `~/.epik/tools`, the private copies of `gh` and Node.
    Tools,
    /// `~/.epik/bin`, the app's link to itself.
    Bin,
    /// `~/.epik/builds`, the build clones.
    Builds,
    /// `~/.epik/runs`, the run history.
    Runs,
    /// `~/.epik` itself, with whatever else is in it.
    Root,
}

impl Step {
    pub const ALL: [Step; 7] = [
        Step::Plugin,
        Step::Marketplace,
        Step::Tools,
        Step::Bin,
        Step::Builds,
        Step::Runs,
        Step::Root,
    ];

    /// What it removes, in words.
    pub fn title(self) -> &'static str {
        match self {
            Step::Plugin => "The Epik plugin",
            Step::Marketplace => "The Epik plugin's marketplace",
            Step::Tools => "Epik's copies of gh and Node",
            Step::Bin => "The link to the app",
            Step::Builds => "Build clones",
            Step::Runs => "Run history",
            Step::Root => "Everything else in ~/.epik",
        }
    }
}

/// How one step went: removed, or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub step: Step,
    pub result: Result<(), String>,
}

/// Removes everything Epik put under `root`, normally `~/.epik`, and its
/// plugin and marketplace from Claude Code, through `runner`. Every step
/// runs even when one before it fails, and what is already gone counts as
/// removed.
pub fn uninstall(root: &Path, runner: &dyn Runner) -> Vec<Outcome> {
    Step::ALL
        .into_iter()
        .map(|step| Outcome {
            step,
            result: remove(root, runner, step),
        })
        .collect()
}

fn remove(root: &Path, runner: &dyn Runner, step: Step) -> Result<(), String> {
    match step {
        Step::Plugin => claude_plugin(root, runner, &["plugin", "uninstall", "epik@epik"]),
        Step::Marketplace => {
            claude_plugin(root, runner, &["plugin", "marketplace", "remove", "epik"])
        }
        Step::Tools => remove_dir(&tools_dir(root)),
        Step::Bin => remove_dir(&root.join("bin")),
        Step::Builds => remove_dir(&builds_dir(root)),
        Step::Runs => remove_dir(&runs_dir(root)),
        Step::Root => remove_dir(root),
    }
}

/// Runs `claude` with `args`. Claude Code saying the plugin or marketplace
/// isn't there means it is already gone.
fn claude_plugin(root: &Path, runner: &dyn Runner, args: &[&str]) -> Result<(), String> {
    let claude = setup::claude(root);
    let out = runner
        .run(&claude, args)
        .map_err(|error| format!("couldn't run claude {}: {error}", args.join(" ")))?;
    let said = format!("{}\n{}", out.stdout, out.stderr);
    let lower = said.to_lowercase();
    if out.success || lower.contains("not found") || lower.contains("not installed") {
        return Ok(());
    }
    let said = said.trim();
    Err(if said.is_empty() {
        format!("claude {} failed", args.join(" "))
    } else {
        said.to_owned()
    })
}

/// Removes the directory at `path` and everything in it. One that is
/// already gone counts as removed.
fn remove_dir(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("couldn't remove {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::Output;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A runner that records each command, written `program args…`, and
    /// answers it with `answer`'s output; a command `answer` doesn't know
    /// isn't installed.
    struct Recording {
        commands: Mutex<Vec<String>>,
        answer: fn(&str) -> Option<Output>,
    }

    impl Recording {
        fn new(answer: fn(&str) -> Option<Output>) -> Self {
            Self {
                commands: Mutex::new(Vec::new()),
                answer,
            }
        }

        fn commands(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
    }

    impl Runner for Recording {
        fn run(&self, program: &Path, args: &[&str]) -> io::Result<Output> {
            let name = program.file_stem().unwrap_or_default().to_string_lossy();
            let command = std::iter::once(name.as_ref())
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.commands.lock().unwrap().push(command.clone());
            (self.answer)(&command).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, command))
        }
    }

    fn output(success: bool, stderr: &str) -> Output {
        Output {
            success,
            stdout: String::new(),
            stderr: stderr.to_owned(),
        }
    }

    /// Claude Code with the plugin and marketplace installed.
    fn installed(command: &str) -> Option<Output> {
        command.starts_with("claude ").then(|| output(true, ""))
    }

    /// Claude Code with neither, answering as Claude Code 2.1 does.
    fn already_gone(command: &str) -> Option<Output> {
        match command {
            "claude plugin uninstall epik@epik" => Some(output(
                false,
                "✘ Failed to uninstall plugin \"epik@epik\": Plugin \"epik@epik\" not found in installed plugins\n",
            )),
            "claude plugin marketplace remove epik" => Some(output(
                false,
                "✘ Failed to remove marketplace: Marketplace 'epik' not found\n",
            )),
            _ => None,
        }
    }

    /// A `~/.epik` as a machine set up by the checklist, with one run.
    fn set_up(home: &Path) -> PathBuf {
        let root = home.join(".epik");
        for dir in [
            "tools/gh/bin",
            "tools/node/bin",
            "bin",
            "builds/acme/widgets/13/clone",
            "runs/acme/widgets/13/20261028T170839Z-k3f9",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("tools/gh/bin/gh"), "").unwrap();
        fs::write(root.join("bin/epik-app"), "").unwrap();
        fs::write(root.join("config.toml"), "").unwrap();
        fs::write(root.join("repositories.json"), "[]").unwrap();
        fs::write(root.join("setup-done"), "").unwrap();
        root
    }

    fn results(outcomes: &[Outcome]) -> Vec<(Step, Result<(), String>)> {
        outcomes
            .iter()
            .map(|outcome| (outcome.step, outcome.result.clone()))
            .collect()
    }

    fn all_removed() -> Vec<(Step, Result<(), String>)> {
        Step::ALL.into_iter().map(|step| (step, Ok(()))).collect()
    }

    #[test]
    fn it_removes_every_part_of_epiks_directory_and_then_the_directory() {
        let home = tempfile::tempdir().unwrap();
        let root = set_up(home.path());

        let outcomes = uninstall(&root, &Recording::new(installed));

        assert_eq!(results(&outcomes), all_removed());
        assert!(!root.exists());
        assert!(home.path().exists());
    }

    #[test]
    fn it_runs_the_two_claude_plugin_commands_in_order_and_nothing_else() {
        let home = tempfile::tempdir().unwrap();
        let root = set_up(home.path());
        let runner = Recording::new(installed);

        uninstall(&root, &runner);

        assert_eq!(
            runner.commands(),
            [
                "claude plugin uninstall epik@epik",
                "claude plugin marketplace remove epik",
            ]
        );
    }

    #[test]
    fn a_plugin_and_marketplace_already_gone_count_as_removed() {
        let home = tempfile::tempdir().unwrap();
        let root = set_up(home.path());

        let outcomes = uninstall(&root, &Recording::new(already_gone));

        assert_eq!(results(&outcomes), all_removed());
    }

    #[test]
    fn an_epik_directory_already_gone_counts_as_removed() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".epik");

        let outcomes = uninstall(&root, &Recording::new(installed));

        assert_eq!(results(&outcomes), all_removed());
    }

    #[test]
    fn parts_of_the_epik_directory_already_gone_count_as_removed() {
        let home = tempfile::tempdir().unwrap();
        let root = set_up(home.path());
        fs::remove_dir_all(root.join("tools")).unwrap();
        fs::remove_dir_all(root.join("builds")).unwrap();

        let outcomes = uninstall(&root, &Recording::new(installed));

        assert_eq!(results(&outcomes), all_removed());
        assert!(!root.exists());
    }

    #[test]
    fn a_failing_step_is_reported_and_the_others_still_run() {
        fn plugin_fails(command: &str) -> Option<Output> {
            match command {
                "claude plugin uninstall epik@epik" => {
                    Some(output(false, "✘ Failed to uninstall plugin: disk full\n"))
                }
                _ => installed(command),
            }
        }
        let home = tempfile::tempdir().unwrap();
        let root = set_up(home.path());
        let runner = Recording::new(plugin_fails);

        let outcomes = uninstall(&root, &runner);

        let mut expected = all_removed();
        expected[0].1 = Err("✘ Failed to uninstall plugin: disk full".to_owned());
        assert_eq!(results(&outcomes), expected);
        assert_eq!(runner.commands().len(), 2);
        assert!(!root.exists());
    }

    #[test]
    fn without_claude_the_plugin_steps_fail_and_the_directory_is_still_removed() {
        let home = tempfile::tempdir().unwrap();
        let root = set_up(home.path());

        let outcomes = uninstall(&root, &Recording::new(|_| None));

        assert_eq!(outcomes[0].step, Step::Plugin);
        assert!(
            outcomes[0]
                .result
                .as_ref()
                .is_err_and(|error| error.starts_with("couldn't run claude plugin uninstall")),
            "{outcomes:?}"
        );
        assert!(outcomes[1].result.is_err());
        assert!(outcomes[2..].iter().all(|outcome| outcome.result.is_ok()));
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_removed_says_why() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let root = set_up(home.path());
        let tools = root.join("tools");
        fs::set_permissions(&tools, fs::Permissions::from_mode(0o500)).unwrap();

        let outcomes = uninstall(&root, &Recording::new(installed));
        fs::set_permissions(&tools, fs::Permissions::from_mode(0o700)).unwrap();

        let failed: Vec<Step> = outcomes
            .iter()
            .filter(|outcome| outcome.result.is_err())
            .map(|outcome| outcome.step)
            .collect();
        assert_eq!(failed, [Step::Tools, Step::Root]);
        assert!(
            outcomes[2]
                .result
                .as_ref()
                .is_err_and(|error| error.starts_with("couldn't remove ")),
            "{outcomes:?}"
        );
        assert!(!root.join("runs").exists());
    }

    #[test]
    fn with_no_epik_directory_there_are_no_runs_to_count() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            crate::run::list(&home.path().join(".epik")).unwrap().len(),
            0
        );
    }
}
