use epik_view::{Change, Entry, Excerpt, LineRange};
use serde_json::{Value, json};

use super::*;

const AT: &str = "2026-10-30T20:07:37.250Z";
/// [`AT`] in milliseconds since the Unix epoch.
const AT_MS: i64 = 1_793_390_857_250;

/// The log's first line: the session, working in `/wt`.
fn session() -> String {
    line(json!({"to_agent": {"method": "session/new", "params": {"cwd": "/wt"}}}))
}

fn line(mut message: Value) -> String {
    message["at"] = json!(AT);
    message.to_string()
}

/// A session update from the agent.
fn update(update: Value) -> String {
    line(
        json!({"from_agent": {"method": "session/update", "params": {"sessionId": "s", "update": update}}}),
    )
}

fn said(text: &str) -> String {
    update(
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}),
    )
}

/// A chunk of the agent's message `message`.
fn said_in(message: &str, text: &str) -> String {
    update(json!({
        "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text},
        "messageId": message,
    }))
}

fn thought(text: &str) -> String {
    update(
        json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": text}}),
    )
}

/// A tool call as the adapter starts it, before its input is known.
fn started(id: &str, tool: &str) -> String {
    update(json!({
        "sessionUpdate": "tool_call", "toolCallId": id, "title": tool, "rawInput": {},
        "status": "pending", "_meta": {"claudeCode": {"toolName": tool}},
    }))
}

/// A tool call's input.
fn input(id: &str, tool: &str, input: Value) -> String {
    update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": id, "rawInput": input,
        "_meta": {"claudeCode": {"toolName": tool}},
    }))
}

/// A tool call's input, as one `tool_call`.
fn call(id: &str, tool: &str, input: Value) -> String {
    update(json!({
        "sessionUpdate": "tool_call", "toolCallId": id, "rawInput": input, "status": "pending",
        "_meta": {"claudeCode": {"toolName": tool}},
    }))
}

fn printed(id: &str, status: &str, output: &str) -> String {
    update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status,
        "rawOutput": output,
    }))
}

fn done(id: &str) -> String {
    update(json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed"}))
}

fn log(lines: &[String]) -> String {
    let mut log = vec![session()];
    log.extend_from_slice(lines);
    log.join("\n")
}

fn entries(lines: &[String]) -> Vec<Entry> {
    parse(&log(lines))
        .entries
        .into_iter()
        .map(|entry| entry.what)
        .collect()
}

fn strings(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

fn all(lines: &[&str]) -> Excerpt {
    Excerpt {
        head: strings(lines),
        ..Excerpt::default()
    }
}

#[test]
fn narration_is_joined_across_chunks() {
    let entries = entries(&[
        said("I'll start "),
        said("by reading the issue."),
        call("t1", "TodoWrite", json!({"todos": []})),
        done("t1"),
        said("Now the tests."),
    ]);

    assert_eq!(
        entries,
        [
            Entry::Narration {
                text: "I'll start by reading the issue.".to_owned()
            },
            Entry::Other {
                text: "Updating its plan".to_owned()
            },
            Entry::Narration {
                text: "Now the tests.".to_owned()
            },
        ]
    );
}

#[test]
fn separate_messages_are_separate_narration() {
    let entries = entries(&[
        said_in("m1", "Reading "),
        said_in("m1", "the issue."),
        said_in("m2", "Now the tests."),
    ]);

    assert_eq!(
        entries,
        [
            Entry::Narration {
                text: "Reading the issue.".to_owned()
            },
            Entry::Narration {
                text: "Now the tests.".to_owned()
            },
        ]
    );
}

#[test]
fn a_thought_between_chunks_separates_the_narration() {
    let entries = entries(&[
        said("Reading the issue."),
        thought("Hmm."),
        said("Now the tests."),
    ]);

    assert_eq!(
        entries,
        [
            Entry::Narration {
                text: "Reading the issue.".to_owned()
            },
            Entry::Narration {
                text: "Now the tests.".to_owned()
            },
        ]
    );
}

/// ACP's tool call updates carry only the fields that changed, and each
/// replaces what was there: a later `rawOutput` is the whole output so far.
#[test]
fn a_commands_output_is_its_latest_raw_output() {
    let command = "cd /wt && cargo test 2>&1 | tail";
    let first = [
        started("t1", "Bash"),
        input("t1", "Bash", json!({"command": command})),
        printed("t1", "in_progress", "Compiling epik\nRunning tests\n"),
    ];
    let mut whole = first.to_vec();
    whole.push(printed(
        "t1",
        "completed",
        "Compiling epik\nRunning tests\ntest result: ok. 3 passed",
    ));

    let running = parse(&log(&first));
    let finished = parse(&log(&whole));

    assert_eq!(
        running.entries[0].what,
        Entry::Command {
            command: command.to_owned(),
            output: all(&["Compiling epik", "Running tests"]),
        }
    );
    assert!(running.working);
    assert_eq!(
        finished.entries[0].what,
        Entry::Command {
            command: command.to_owned(),
            output: all(&[
                "Compiling epik",
                "Running tests",
                "test result: ok. 3 passed"
            ]),
        }
    );
    assert!(!finished.working);
}

#[test]
fn a_commands_output_may_come_as_text_blocks() {
    let entries = entries(&[
        call("t1", "Bash", json!({"command": "ls"})),
        update(json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
            "rawOutput": [{"type": "text", "text": "a.rs\nb.rs"}],
        })),
    ]);

    assert_eq!(
        entries,
        [Entry::Command {
            command: "ls".to_owned(),
            output: all(&["a.rs", "b.rs"]),
        }]
    );
}

#[test]
fn a_read_names_its_path_and_any_range() {
    let read = |input| entries(&[call("t1", "Read", input), done("t1")]).remove(0);

    assert_eq!(
        read(json!({"file_path": "/wt/src/eval.rs"})),
        Entry::Read {
            path: "src/eval.rs".to_owned(),
            lines: None
        }
    );
    assert_eq!(
        read(json!({"file_path": "/wt/src/eval.rs", "offset": 10, "limit": 5})),
        Entry::Read {
            path: "src/eval.rs".to_owned(),
            lines: Some(LineRange {
                from: 10,
                to: Some(14)
            })
        }
    );
    assert_eq!(
        read(json!({"file_path": "/wt/src/eval.rs", "offset": 110})),
        Entry::Read {
            path: "src/eval.rs".to_owned(),
            lines: Some(LineRange {
                from: 110,
                to: None
            })
        }
    );
    assert_eq!(
        read(json!({"file_path": "/elsewhere/notes.md", "limit": 20})),
        Entry::Read {
            path: "/elsewhere/notes.md".to_owned(),
            lines: Some(LineRange {
                from: 1,
                to: Some(20)
            })
        }
    );
}

#[test]
fn a_search_names_its_matches() {
    let search = |input, output: &str| {
        entries(&[
            call("t1", "Grep", input),
            printed("t1", "completed", output),
        ])
        .remove(0)
    };

    assert_eq!(
        search(
            json!({"pattern": "fn eval", "path": "/wt/src", "output_mode": "content", "-n": true}),
            "/wt/src/eval.rs:3:fn eval() {}\nsrc/lib.rs:7:    fn eval_all() {}",
        ),
        Entry::Search {
            pattern: "fn eval".to_owned(),
            within: Some("src".to_owned()),
            matches: 2,
            hits: all(&["src/eval.rs:3", "src/lib.rs:7"]),
        }
    );
    assert_eq!(
        search(
            json!({"pattern": "TODO"}),
            "Found 2 files\n/wt/src/a.rs\n/wt/src/b.rs"
        ),
        Entry::Search {
            pattern: "TODO".to_owned(),
            within: None,
            matches: 2,
            hits: all(&["src/a.rs", "src/b.rs"]),
        }
    );
    assert_eq!(
        search(json!({"pattern": "TODO", "glob": "*.rs"}), "No files found"),
        Entry::Search {
            pattern: "TODO".to_owned(),
            within: Some("*.rs".to_owned()),
            matches: 0,
            hits: Excerpt::default(),
        }
    );
    assert_eq!(
        entries(&[
            call("t1", "Glob", json!({"pattern": "**/*.toml"})),
            printed("t1", "completed", "/wt/Cargo.toml"),
        ])
        .remove(0),
        Entry::Search {
            pattern: "**/*.toml".to_owned(),
            within: None,
            matches: 1,
            hits: all(&["Cargo.toml"]),
        }
    );
}

#[test]
fn a_search_still_running_has_no_matches_yet() {
    let stream = parse(&log(&[call("t1", "Grep", json!({"pattern": "x"}))]));

    assert_eq!(
        stream.entries[0].what,
        Entry::Search {
            pattern: "x".to_owned(),
            within: None,
            matches: 0,
            hits: Excerpt::default(),
        }
    );
    assert!(stream.working);
}

#[test]
fn an_edit_shows_the_lines_it_changed() {
    let removed = |text: &str| Change::Removed(text.to_owned());
    let added = |text: &str| Change::Added(text.to_owned());
    let edit = |lines: &[String]| entries(lines).remove(0);
    let changes = |changes: Vec<Change>| Entry::Edit {
        path: "src/eval.rs".to_owned(),
        changes: Excerpt {
            head: changes,
            ..Excerpt::default()
        },
    };

    // From its input, without the lines around the change.
    assert_eq!(
        edit(&[call(
            "t1",
            "Edit",
            json!({"file_path": "/wt/src/eval.rs", "old_string": "a\nb\nc", "new_string": "a\nB\nB2\nc"}),
        )]),
        changes(vec![removed("b"), added("B"), added("B2")])
    );
    // From what Claude Code says it did, when the input leaves it out.
    assert_eq!(
        edit(&[
            call("t1", "Edit", json!({"file_path": "/wt/src/eval.rs"})),
            update(json!({
                "sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "_meta": {"claudeCode": {"toolName": "Edit", "toolResponse": {
                    "filePath": "/wt/src/eval.rs", "oldString": "x", "newString": "y",
                    "structuredPatch": [{"oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
                        "lines": [" a", "-x", "+y", " c"]}],
                }}},
            })),
        ]),
        changes(vec![removed("x"), added("y")])
    );
    // Each of a MultiEdit's edits.
    assert_eq!(
        edit(&[call(
            "t1",
            "MultiEdit",
            json!({"file_path": "/wt/src/eval.rs", "edits": [
                {"old_string": "a", "new_string": "b"},
                {"old_string": "c", "new_string": "d"},
            ]}),
        )]),
        changes(vec![removed("a"), added("b"), removed("c"), added("d")])
    );
}

#[test]
fn a_new_file_shows_its_first_lines() {
    let entries = entries(&[call(
        "t1",
        "Write",
        json!({"file_path": "/wt/src/power.rs", "content": "//! Powers.\n\npub fn pow() {}\n"}),
    )]);

    assert_eq!(
        entries,
        [Entry::NewFile {
            path: "src/power.rs".to_owned(),
            content: all(&["//! Powers.", "", "pub fn pow() {}"]),
        }]
    );
}

#[test]
fn long_output_keeps_its_first_and_last_lines() {
    let output: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();

    let entries = entries(&[
        call("t1", "Bash", json!({"command": "seq 30"})),
        printed("t1", "completed", &output.join("\n")),
    ]);

    let Entry::Command { output: cut, .. } = &entries[0] else {
        panic!("{entries:?}");
    };
    assert_eq!(cut.head, output[..6]);
    assert_eq!(cut.more, 19);
    assert_eq!(cut.tail, output[25..]);
}

#[test]
fn output_at_the_limit_is_kept_whole() {
    let output: Vec<String> = (1..=LIMIT).map(|n| format!("line {n}")).collect();

    assert_eq!(
        cut(output.clone()),
        Excerpt {
            head: output,
            ..Excerpt::default()
        }
    );
}

#[test]
fn an_entry_has_the_time_it_began() {
    let stream = parse(&log(&[
        said("Reading."),
        said("Still reading."),
        call("t1", "Read", json!({"file_path": "/wt/a.rs"})),
    ]));

    assert_eq!(stream.entries[0].at, Some(AT_MS));
    assert_eq!(stream.entries[1].at, Some(AT_MS));
}

#[test]
fn a_line_without_a_time_gives_an_entry_without_one() {
    let untimed = json!({"from_agent": {"params": {"update":
        {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Hello."}}}}});

    let stream = parse(&untimed.to_string());

    assert_eq!(
        stream.entries,
        [StreamEntry {
            at: None,
            what: Entry::Narration {
                text: "Hello.".to_owned()
            },
        }]
    );
}

#[test]
fn a_malformed_line_is_skipped() {
    let text = [said("One."), "{\"from_agent\": {".to_owned(), said(" Two.")].join("\n");

    let stream = parse(&text);

    assert_eq!(
        stream.entries.iter().map(|e| &e.what).collect::<Vec<_>>(),
        [&Entry::Narration {
            text: "One. Two.".to_owned()
        }]
    );
}

#[test]
fn the_agent_is_working_until_its_last_tool_call_completes() {
    assert!(!parse("").working);
    assert!(!parse(&log(&[said("Done.")])).working);
    assert!(parse(&log(&[call("t1", "Read", json!({"file_path": "/wt/a"}))])).working);
    assert!(
        !parse(&log(&[
            call("t1", "Edit", json!({"file_path": "/wt/a"})),
            printed(
                "t1",
                "failed",
                "<tool_use_error>File has not been read yet.</tool_use_error>"
            ),
        ]))
        .working
    );
}

#[test]
fn a_recorded_log_reads_as_a_stream() {
    let path = format!("{}/src/fixtures/agent.jsonl", env!("CARGO_MANIFEST_DIR"));
    let stream = read(Path::new(&path)).unwrap();

    let kinds: Vec<String> = stream
        .entries
        .iter()
        .map(|entry| match &entry.what {
            Entry::Narration { text } => format!("said {text}"),
            Entry::Command { command, .. } => format!("ran {command}"),
            Entry::Read { path, .. } => format!("read {path}"),
            Entry::Edit { path, .. } => format!("edited {path}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "said I'll start by reading the issue.",
            "ran gh issue view 14",
            "ran cd /home/me/.epik/builds/owner/repo/13/20261028T170000Z-k3f9 && git worktree add -b issue-14 ../wt-14 origin/epic/13",
            "read src/lexer.rs",
            "edited src/eval.rs",
            "ran cd /home/me/.epik/builds/owner/repo/13/wt-14 && cargo test 2>&1 | tail -5",
            "edited src/eval.rs",
            "ran cd /home/me/.epik/builds/owner/repo/13/wt-14 && cargo build --release",
        ]
    );
    assert!(stream.entries.iter().all(|entry| entry.at.is_none()));
    assert!(stream.working);
}
