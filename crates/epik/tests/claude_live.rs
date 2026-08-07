//! The acceptance run: real Claude Code, one `Implement` task, end to end.
//!
//! This tier is ignored by default rather than gated the live-model way,
//! because its precondition is different in kind: `tests/live.rs` needs a
//! free local server anyone can start, while this needs a logged-in `claude`
//! and spends real money. CI must never run it — the live job's
//! `EPIK_REQUIRE_LIVE=1` would otherwise turn "no claude on the runner"
//! into a permanent failure — and a laptop should run it on purpose:
//!
//! ```sh
//! cargo test -p epik --test claude_live -- --ignored --nocapture
//! ```
//!
//! What is asserted is the contract, never prose quality: the run starts,
//! narrates, bills cumulatively, finishes exactly once with `Completed`,
//! and the work — one file, stated exactly — is really on disk.

// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::time::Duration;

use epik::agent::{AgentEvent, Budget, ClaudeCode, CodingAgent, Stop, Task, TaskKind};
use epik::chat::StopToken;
use epik::event::Money;

#[test]
#[ignore = "needs a logged-in claude and spends real money; run with -- --ignored"]
fn claude_code_implements_a_task_end_to_end() {
    let worktree = tempfile::tempdir().unwrap();
    let task = Task {
        kind: TaskKind::Implement,
        prompt: "Create a file named HELLO.txt containing exactly the text `hello from epik`. \
                 Do nothing else: no other files, no questions, no summary document."
            .to_owned(),
        worktree: worktree.path().to_owned(),
        env: Vec::new(),
        budget: Budget {
            max_tokens: None,
            // A trivial task that bills two dollars has gone wrong enough
            // to kill.
            max_cost: Some(Money {
                micro_usd: 2_000_000,
            }),
            // Roomy: real claude thinks quietly between lines.
            stall: Duration::from_mins(5),
        },
    };
    let agent = ClaudeCode::new();
    let mut events = Vec::new();
    let mut sink = |event: AgentEvent| {
        println!("{event:?}");
        events.push(event);
    };

    let stopped = agent
        .run(&task, &mut sink, &StopToken::new())
        .expect("a logged-in claude on PATH conducts the run");

    assert_eq!(stopped, Stop::Completed, "events: {events:?}");
    assert!(
        matches!(events.first(), Some(AgentEvent::Started { .. })),
        "{events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Finished(Stop::Completed)),
        "one Finished, last, agreeing with the returned stop"
    );
    let text = fs::read_to_string(worktree.path().join("HELLO.txt"))
        .expect("the task's one deliverable exists");
    assert!(text.contains("hello from epik"), "{text:?}");
    let billed = events
        .iter()
        .rev()
        .find_map(|event| match event {
            AgentEvent::Usage(usage) => Some(*usage),
            _ => None,
        })
        .expect("a real run bills something");
    assert!(billed.total_tokens() > 0, "{billed:?}");
    assert!(
        billed.cost.is_some(),
        "Claude Code states its spend in dollars: {billed:?}"
    );
}
