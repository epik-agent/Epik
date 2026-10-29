//! What a running agent is doing, read from its log.
//!
//! Epik works for minutes at a time, so the window shows proof of life
//! without being asked: each running issue's last action, its last test
//! result, and when its agent last logged anything. [`read`] gets these
//! from the agent's `<issue>.jsonl`, the ACP messages [`crate::agent`]
//! records, by following its tool calls. It says only what the agent did,
//! in words; it never guesses what the agent is trying to do.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How many of the agent's actions [`Activity::recent`] keeps.
pub const RECENT: usize = 30;

/// What a running agent is doing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    /// Its latest tool call, in words: "Editing src/eval.rs", "Running
    /// cargo test".
    pub last_action: Option<String>,
    /// From the latest test command's output: "23 passed, 1 failed". None
    /// before any test has run.
    pub last_test: Option<String>,
    /// When its log last grew.
    pub last_event: Option<DateTime<Utc>>,
    /// Its latest actions in words, oldest first, at most [`RECENT`].
    pub recent: Vec<String>,
    /// What its session has cost so far, in US dollars, from its latest
    /// usage update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// The activity in the agent log at `path`.
pub fn read(path: &Path) -> io::Result<Activity> {
    let text = fs::read_to_string(path)?;
    let modified = fs::metadata(path)?.modified()?;
    Ok(Activity {
        last_event: Some(modified.into()),
        ..parse(&text)
    })
}

/// A tool call, as its updates arrive.
#[derive(Default)]
struct Call {
    tool: Option<String>,
    title: Option<String>,
    input: Option<Value>,
}

/// The activity in an agent log's text, without when it last grew.
pub fn parse(text: &str) -> Activity {
    let mut dirs: Vec<String> = Vec::new();
    let mut calls: HashMap<String, Call> = HashMap::new();
    // The calls with their input known, in the order it arrived.
    let mut actions: Vec<String> = Vec::new();
    let mut last_test = None;
    let mut cost_usd = None;
    for line in text.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(cwd) = message
            .pointer("/to_agent/params/cwd")
            .and_then(Value::as_str)
        {
            dirs.push(cwd.to_owned());
            continue;
        }
        let Some(update) = message.pointer("/from_agent/params/update") else {
            continue;
        };
        let kind = update.get("sessionUpdate").and_then(Value::as_str);
        if kind == Some("usage_update")
            && update.pointer("/cost/currency").and_then(Value::as_str) == Some("USD")
        {
            cost_usd = update.pointer("/cost/amount").and_then(Value::as_f64);
        }
        if !matches!(kind, Some("tool_call" | "tool_call_update")) {
            continue;
        }
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            continue;
        };
        let call = calls.entry(id.to_owned()).or_default();
        if let Some(tool) = update.pointer("/_meta/claudeCode/toolName") {
            call.tool = tool.as_str().map(str::to_owned);
        }
        if let Some(title) = update.get("title").and_then(Value::as_str) {
            call.title = Some(title.to_owned());
        }
        if let Some(input) = update.get("rawInput")
            && input.as_object().is_some_and(|input| !input.is_empty())
        {
            if call.input.is_none() {
                actions.push(id.to_owned());
            }
            call.input = Some(input.clone());
            if let Some(command) = input.get("command").and_then(Value::as_str) {
                dirs.extend(cd_targets(command));
            }
        }
        if update.get("status").and_then(Value::as_str) == Some("completed")
            && let Some(command) = call
                .input
                .as_ref()
                .and_then(|input| input.get("command"))
                .and_then(Value::as_str)
            && is_test_command(command)
            && let Some(result) = test_result(&output(update))
        {
            last_test = Some(result);
        }
    }
    // Described once every directory the agent works in is known, so a
    // file read before the agent first `cd`s into its worktree is still
    // named relative to it.
    let skip = actions.len().saturating_sub(RECENT);
    let recent: Vec<String> = actions[skip..]
        .iter()
        .map(|id| describe(&calls[id], &dirs))
        .collect();
    Activity {
        last_action: recent.last().cloned(),
        last_test,
        last_event: None,
        recent,
        cost_usd,
    }
}

/// A completed tool call's output: its `rawOutput` if that is text, else
/// what Claude Code says the tool printed.
fn output(update: &Value) -> String {
    if let Some(text) = update.get("rawOutput").and_then(Value::as_str) {
        return text.to_owned();
    }
    ["stdout", "stderr"]
        .iter()
        .filter_map(|stream| {
            update
                .pointer(&format!("/_meta/claudeCode/toolResponse/{stream}"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool call in words.
fn describe(call: &Call, dirs: &[String]) -> String {
    let input = call.input.as_ref();
    let field = |name: &str| input.and_then(|i| i.get(name)).and_then(Value::as_str);
    let path = || {
        field("file_path")
            .or_else(|| field("notebook_path"))
            .map(|p| relative(p, dirs))
    };
    match call.tool.as_deref() {
        Some("Bash") => match field("command") {
            Some(command) => format!("Running {}", program(command)),
            None => "Running a command".to_owned(),
        },
        Some("Edit" | "MultiEdit" | "NotebookEdit") => match path() {
            Some(path) => format!("Editing {path}"),
            None => "Editing".to_owned(),
        },
        Some("Write") => match path() {
            Some(path) => format!("Writing {path}"),
            None => "Writing".to_owned(),
        },
        Some("Read") => match path() {
            Some(path) => format!("Reading {path}"),
            None => "Reading".to_owned(),
        },
        Some("Grep" | "Glob") => match field("pattern") {
            Some(pattern) => format!("Searching for {pattern}"),
            None => "Searching".to_owned(),
        },
        Some("Task" | "Agent") => "Running a subagent".to_owned(),
        Some("TodoWrite") => "Updating its plan".to_owned(),
        Some("WebFetch" | "WebSearch") => "Reading the web".to_owned(),
        Some("Skill") => match field("skill") {
            Some(skill) => format!("Using {skill}"),
            None => "Using a skill".to_owned(),
        },
        _ => call
            .title
            .clone()
            .or_else(|| call.tool.clone())
            .unwrap_or_else(|| "Working".to_owned()),
    }
}

/// `path` relative to the longest of `dirs` it is inside, or as it is.
fn relative(path: &str, dirs: &[String]) -> String {
    dirs.iter()
        .filter_map(|dir| {
            let dir = dir.trim_end_matches('/');
            path.strip_prefix(dir)?.strip_prefix('/')
        })
        .min_by_key(|rest| rest.len())
        .unwrap_or(path)
        .to_owned()
}

/// The parts of a shell command, split at `&&`, `||`, `;`, `|` and new
/// lines. Good enough to name what a command runs; not a shell parser.
fn segments(command: &str) -> Vec<&str> {
    command
        .split(['\n', ';', '|', '&'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// The absolute directories a command `cd`s into.
fn cd_targets(command: &str) -> Vec<String> {
    segments(command)
        .into_iter()
        .filter_map(|segment| segment.strip_prefix("cd "))
        .map(|dir| dir.trim().trim_matches(['"', '\'']).to_owned())
        .filter(|dir| dir.starts_with('/'))
        .collect()
}

/// What a command runs: its first program that isn't `cd`, and that
/// program's subcommand if it has one: "cargo test", "git worktree", "cat".
fn program(command: &str) -> String {
    let segment = segments(command)
        .into_iter()
        .find(|segment| !segment.starts_with("cd ") && *segment != "cd")
        .unwrap_or(command);
    let mut words = segment
        .split_whitespace()
        .skip_while(|word| word.contains('=') && !word.starts_with('-'));
    let Some(program) = words.next() else {
        return "a command".to_owned();
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    match words.next() {
        Some(sub)
            if sub
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '_'))
                && !sub.starts_with('-') =>
        {
            format!("{program} {sub}")
        }
        _ => program.to_owned(),
    }
}

/// The commands that run tests.
const TEST_COMMANDS: [&str; 14] = [
    "cargo test",
    "cargo nextest",
    "npm test",
    "npm run test",
    "pnpm test",
    "yarn test",
    "bun test",
    "pytest",
    "go test",
    "jest",
    "vitest",
    "node --test",
    "make test",
    "mix test",
];

fn is_test_command(command: &str) -> bool {
    segments(command).iter().any(|segment| {
        let words = segment.split_whitespace().collect::<Vec<_>>().join(" ");
        TEST_COMMANDS.iter().any(|test| words.contains(test))
    })
}

/// "23 passed, 1 failed" from a test command's output, summing every count
/// it prints, as `cargo test` does once per test binary. None if it prints
/// no counts.
fn test_result(output: &str) -> Option<String> {
    let words: Vec<&str> = output
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')'))
        .filter(|word| !word.is_empty())
        .collect();
    let (mut passed, mut failed, mut found) = (0u64, 0u64, false);
    for pair in words.windows(2) {
        let Ok(count) = pair[0].parse::<u64>() else {
            continue;
        };
        if pair[1].starts_with("passed") {
            passed += count;
            found = true;
        } else if pair[1].starts_with("failed") {
            failed += count;
            found = true;
        }
    }
    found.then(|| match failed {
        0 => format!("{passed} passed"),
        _ => format!("{passed} passed, {failed} failed"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        let path = format!("{}/src/fixtures/agent.jsonl", env!("CARGO_MANIFEST_DIR"));
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    #[test]
    fn a_recorded_log_gives_the_last_action_and_test_result() {
        let activity = parse(&fixture());

        assert_eq!(activity.last_action.as_deref(), Some("Running cargo build"));
        assert_eq!(activity.last_test.as_deref(), Some("25 passed, 1 failed"));
        assert_eq!(activity.cost_usd, Some(0.42));
        assert_eq!(
            activity.recent,
            [
                "Running gh issue",
                "Running git worktree",
                "Reading src/lexer.rs",
                "Editing src/eval.rs",
                "Running cargo test",
                "Editing src/eval.rs",
                "Running cargo build",
            ]
        );
    }

    #[test]
    fn the_last_event_is_when_the_log_last_grew() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("14.jsonl");
        fs::write(&path, fixture()).unwrap();
        let modified: DateTime<Utc> = fs::metadata(&path).unwrap().modified().unwrap().into();

        let activity = read(&path).unwrap();

        assert_eq!(activity.last_event, Some(modified));
        assert_eq!(activity.last_action.as_deref(), Some("Running cargo build"));
    }

    #[test]
    fn a_log_with_no_tool_calls_has_no_action_and_no_tests() {
        let activity = parse("{\"to_agent\":{\"method\":\"initialize\"}}\nnot json\n");

        assert_eq!(activity, Activity::default());
    }

    #[test]
    fn only_the_latest_actions_are_kept() {
        let line = |n: usize| {
            format!(
                r#"{{"from_agent":{{"params":{{"update":{{"sessionUpdate":"tool_call","toolCallId":"{n}","rawInput":{{"command":"step{n}"}},"_meta":{{"claudeCode":{{"toolName":"Bash"}}}}}}}}}}}}"#
            )
        };
        let text: Vec<String> = (0..RECENT + 5).map(line).collect();

        let activity = parse(&text.join("\n"));

        assert_eq!(activity.recent.len(), RECENT);
        assert_eq!(activity.recent[0], "Running step5");
        assert_eq!(
            activity.last_action,
            Some(format!("Running step{}", RECENT + 4))
        );
    }

    #[test]
    fn a_command_is_named_by_its_program_and_subcommand() {
        assert_eq!(program("cargo test -p epik-core"), "cargo test");
        assert_eq!(program("cd /x && cargo build 2>&1 | tail"), "cargo build");
        assert_eq!(program("cat /etc/hosts"), "cat");
        assert_eq!(program("RUST_LOG=1 cargo run"), "cargo run");
        assert_eq!(program("/usr/bin/git status"), "git status");
        assert_eq!(program("npm --prefix x test"), "npm");
        assert_eq!(program(""), "a command");
    }

    #[test]
    fn test_commands_are_recognized() {
        for command in [
            "cargo test",
            "cd /x && cargo   test -p a 2>&1 | tail",
            "npm test",
            "python -m pytest -q",
            "npx vitest run",
            "go test ./...",
        ] {
            assert!(is_test_command(command), "{command}");
        }
        for command in ["cargo build", "git commit -m 'test'", "cat test.rs"] {
            assert!(!is_test_command(command), "{command}");
        }
    }

    #[test]
    fn test_counts_are_summed_across_a_commands_output() {
        let cargo = "test result: ok. 142 passed; 0 failed; 0 ignored\n\
                     test result: ok. 9 passed; 0 failed; 0 ignored";
        assert_eq!(test_result(cargo).as_deref(), Some("151 passed"));
        let jest = "Tests:       1 failed, 23 passed, 24 total";
        assert_eq!(test_result(jest).as_deref(), Some("23 passed, 1 failed"));
        let pytest = "==== 3 passed, 2 failed in 0.12s ====";
        assert_eq!(test_result(pytest).as_deref(), Some("3 passed, 2 failed"));
        assert_eq!(test_result("error: could not compile"), None);
    }

    #[test]
    fn paths_are_shown_relative_to_where_the_agent_works() {
        let dirs = ["/b/clone".to_owned(), "/b/wt-14/".to_owned()];
        assert_eq!(relative("/b/wt-14/src/eval.rs", &dirs), "src/eval.rs");
        assert_eq!(relative("/b/clone/README.md", &dirs), "README.md");
        assert_eq!(relative("/elsewhere/x.rs", &dirs), "/elsewhere/x.rs");
    }
}
