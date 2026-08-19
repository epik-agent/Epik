//! Green is whatever the repository says green is.
//!
//! A [`Check`] holds the one command a repository judges itself by.
//! [`run`] executes it in the feature workspace through `sh -c` — real
//! check commands are pipelines — and reads the exit code; Epik has no
//! opinion about what the command is. A shell here is not the weak
//! link: a coding Agent with permissions skipped is already running in
//! that same worktree.
//!
//! [`detect`] is a fixed table over markers in the worktree, and it
//! only proposes: detection prefills, it does not decide, and the
//! command in force is the one it is handed. A repository matching no
//! marker proposes nothing, and skipping the check is a first-class
//! answer taken elsewhere — the build then runs on observation alone
//! and says so.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// A repository's own idea of green: a command, judged by its exit code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Check {
    pub command: String,
}

/// How long a check may run before it is killed and judged red.
/// Generous, because a check is a whole test suite; a bound all the
/// same, because the check runs under the merge lock, and a hung check
/// — a deadlocked test, a tool that prompts despite a closed stdin —
/// must not hold every future merge hostage.
pub const DEADLINE: Duration = Duration::from_secs(60 * 60);

/// What one run of the check said: green or not, and the command's own
/// words — stdout and stderr both — for the record when it is not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verdict {
    pub green: bool,
    pub output: String,
}

/// Runs `check` in `workspace` through `sh -c` and judges the exit
/// code. A shell that cannot be started, a command killed by a signal,
/// or one still running at the [`DEADLINE`] is nobody's green; the
/// verdict says why in words.
#[must_use]
pub fn run(check: &Check, workspace: &Path) -> Verdict {
    run_within(check, workspace, DEADLINE)
}

/// [`run`] against a stated deadline — which is how the tests exercise
/// the kill without sitting through the real one.
fn run_within(check: &Check, workspace: &Path, deadline: Duration) -> Verdict {
    let mut command = Command::new("sh");
    command.arg("-c").arg(&check.command).current_dir(workspace);
    match crate::child::run("the check", &mut command, deadline) {
        Ok(finished) => Verdict {
            green: finished.success,
            output: finished.output,
        },
        Err(words) => Verdict {
            green: false,
            output: words,
        },
    }
}

/// The detection table: the first marker in `worktree` that names a
/// check proposes its command; a worktree matching none proposes
/// nothing. A proposal, always — the command in force is the one the
/// build is handed.
#[must_use]
pub fn detect(worktree: &Path) -> Option<Check> {
    let marker = |name: &str| worktree.join(name).is_file();
    let command = if npm_test(worktree) {
        "npm test"
    } else if marker("go.mod") {
        "go test ./..."
    } else if marker("pyproject.toml") {
        "python -m pytest"
    } else if marker("pom.xml") {
        "mvn test"
    } else if make_test(worktree) {
        "make test"
    } else if marker("Cargo.toml") {
        "cargo test"
    } else {
        return None;
    };
    Some(Check {
        command: command.to_owned(),
    })
}

/// A `package.json` whose `scripts.test` is a string — the marker for
/// `npm test`. A manifest without one falls through to the next arm.
fn npm_test(worktree: &Path) -> bool {
    std::fs::read_to_string(worktree.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|manifest| manifest["scripts"]["test"].is_string())
}

/// A `Makefile` with a `test` target: a line that begins `test`, then
/// its colon — and not `:=`, which would be a variable wearing the name.
fn make_test(worktree: &Path) -> bool {
    std::fs::read_to_string(worktree.join("Makefile")).is_ok_and(|text| {
        text.lines().any(|line| {
            line.strip_prefix("test")
                .map(str::trim_start)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| !rest.starts_with('='))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that cleans up after itself.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "epik-check-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, content: &str) -> &Self {
            std::fs::write(self.0.join(name), content).unwrap();
            self
        }

        fn proposes(&self, command: &str) {
            assert_eq!(
                detect(&self.0),
                Some(Check {
                    command: command.to_owned()
                })
            );
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_zero_exit_is_green() {
        let scratch = Scratch::new("green");
        let verdict = run(
            &Check {
                command: "echo checked".to_owned(),
            },
            &scratch.0,
        );
        assert!(verdict.green);
        assert_eq!(verdict.output, "checked\n");
    }

    #[test]
    fn anything_else_is_red_with_the_commands_own_words() {
        let scratch = Scratch::new("red");
        let verdict = run(
            &Check {
                command: "echo it broke >&2; exit 3".to_owned(),
            },
            &scratch.0,
        );
        assert!(!verdict.green);
        assert!(verdict.output.contains("it broke"), "{}", verdict.output);
    }

    #[test]
    fn the_check_is_a_shell_command_run_in_the_workspace() {
        let scratch = Scratch::new("where");
        scratch.write("marker.txt", "here\n");
        // A pipeline, deliberately: real check commands are pipelines.
        let verdict = run(
            &Check {
                command: "cat marker.txt | tr -d '\\n'".to_owned(),
            },
            &scratch.0,
        );
        assert!(verdict.green);
        assert_eq!(verdict.output, "here");
    }

    /// The deadline, on a short fuse rather than the real hour: a check
    /// that will not finish is killed and judged red in words, so a hung
    /// suite cannot hold the merge lock forever.
    #[test]
    fn a_check_that_will_not_finish_is_killed_and_judged_red() {
        let scratch = Scratch::new("hung");
        let verdict = run_within(
            &Check {
                command: "sleep 60".to_owned(),
            },
            &scratch.0,
            Duration::from_millis(200),
        );
        assert!(!verdict.green);
        assert!(verdict.output.contains("killed"), "{}", verdict.output);
    }

    #[test]
    fn a_package_json_with_a_test_script_proposes_npm_test() {
        let scratch = Scratch::new("npm");
        scratch.write("package.json", r#"{"scripts": {"test": "vitest run"}}"#);
        scratch.proposes("npm test");
    }

    #[test]
    fn a_package_json_without_a_test_script_is_no_marker() {
        let scratch = Scratch::new("npmless");
        scratch.write("package.json", r#"{"scripts": {"build": "tsc"}}"#);
        assert_eq!(detect(&scratch.0), None);
    }

    #[test]
    fn a_go_mod_proposes_go_test() {
        let scratch = Scratch::new("go");
        scratch.write("go.mod", "module example.com/wumpus\n");
        scratch.proposes("go test ./...");
    }

    #[test]
    fn a_pyproject_proposes_pytest() {
        let scratch = Scratch::new("python");
        scratch.write("pyproject.toml", "[project]\nname = \"wumpus\"\n");
        scratch.proposes("python -m pytest");
    }

    #[test]
    fn a_pom_proposes_mvn_test() {
        let scratch = Scratch::new("maven");
        scratch.write("pom.xml", "<project/>\n");
        scratch.proposes("mvn test");
    }

    #[test]
    fn a_makefile_with_a_test_target_proposes_make_test() {
        let scratch = Scratch::new("make");
        scratch.write("Makefile", "build:\n\tcc main.c\n\ntest: build\n\t./run\n");
        scratch.proposes("make test");
    }

    #[test]
    fn a_makefile_without_a_test_target_is_no_marker() {
        let scratch = Scratch::new("makeless");
        // `tests:` is a different target and `test :=` is a variable.
        scratch.write("Makefile", "tests:\n\t./run\ntest := nope\n");
        assert_eq!(detect(&scratch.0), None);
    }

    #[test]
    fn a_cargo_toml_proposes_cargo_test() {
        let scratch = Scratch::new("cargo");
        scratch.write("Cargo.toml", "[package]\nname = \"wumpus\"\n");
        scratch.proposes("cargo test");
    }

    #[test]
    fn a_worktree_matching_no_marker_proposes_nothing() {
        let scratch = Scratch::new("nothing");
        scratch.write("README.md", "# wumpus\n");
        assert_eq!(detect(&scratch.0), None);
    }

    #[test]
    fn the_first_marker_in_table_order_wins() {
        let scratch = Scratch::new("order");
        scratch
            .write("package.json", r#"{"scripts": {"test": "vitest run"}}"#)
            .write("Cargo.toml", "[package]\nname = \"wumpus\"\n");
        scratch.proposes("npm test");
    }
}
