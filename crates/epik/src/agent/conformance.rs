//! The contract, enforced: hostile scripts every [`CodingAgent`] must
//! survive.
//!
//! Governance lives inside each implementation — a black box mid-`run`
//! cannot be killed from outside — which means nothing but a test can hold a
//! wrapper to the budget, the stall window, and the terminal `Finished`.
//! This module is that test, `pub` so the next implementation runs the same
//! gauntlet: a wrapper around a real binary conforms by translating each
//! [`Play`](crate::agent::Play) into the feed its agent reads. CI runs
//! [`conforms`] against
//! every implementation, permanently; the contract is machinery, not review
//! vigilance.

use std::path::PathBuf;
use std::time::Duration;

use crate::agent::{AgentEvent, Budget, CodingAgent, Play, Script, Stop, Task, TaskKind};
use crate::chat::StopToken;
use crate::event::{Money, Usage};

/// A minute of everything: roomy enough that no benign script trips it, so
/// each check tightens exactly the ceiling it is about.
const fn roomy() -> Budget {
    Budget {
        max_tokens: Some(1_000_000),
        max_cost: Some(Money {
            micro_usd: 100_000_000,
        }),
        stall: Duration::from_mins(1),
    }
}

fn task(budget: Budget) -> Task {
    Task {
        kind: TaskKind::Implement,
        prompt: "implement issue #0".to_owned(),
        worktree: PathBuf::from("."),
        env: Vec::new(),
        budget,
    }
}

/// Runs the agent and collects its stream, then asserts what every run must
/// satisfy regardless of script: `Started` first, exactly one `Finished`,
/// nothing after it, the returned [`Stop`] agreeing with it, and every
/// emitted [`Usage`] monotone nondecreasing. Returns the stop for the
/// caller's own assertion.
fn run(agent: &impl CodingAgent, task: &Task, stop: &StopToken) -> (Vec<AgentEvent>, Stop) {
    let mut events = Vec::new();
    let mut sink = |event: AgentEvent| events.push(event);
    let result = agent.run(task, &mut sink, stop);
    let stopped = match result {
        Ok(stopped) => stopped,
        Err(error) => panic!("a conformance run never errors, but got: {error}"),
    };
    assert!(
        matches!(events.first(), Some(AgentEvent::Started { .. })),
        "a run names its agent before anything else: {events:?}"
    );
    let finishes: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(at, event)| matches!(event, AgentEvent::Finished(_)).then_some(at))
        .collect();
    assert_eq!(
        finishes.len(),
        1,
        "exactly one Finished per run: {events:?}"
    );
    assert_eq!(
        finishes[0],
        events.len() - 1,
        "nothing is emitted after Finished: {events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Finished(stopped.clone())),
        "the returned stop and the Finished event tell one story"
    );
    let readings: Vec<&Usage> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Usage(usage) => Some(usage),
            _ => None,
        })
        .collect();
    for pair in readings.windows(2) {
        assert!(
            nondecreasing(pair[0], pair[1]),
            "usage totals are cumulative and never run backward: {readings:?}"
        );
    }
    (events, stopped)
}

/// Whether `later` reads at least `earlier` in every denomination — a count
/// or a cost, once reported, neither shrinks nor disappears.
fn nondecreasing(earlier: &Usage, later: &Usage) -> bool {
    later.prompt_tokens >= earlier.prompt_tokens
        && later.completion_tokens >= earlier.completion_tokens
        && later.cache_tokens.unwrap_or(0) >= earlier.cache_tokens.unwrap_or(0)
        && match (earlier.cost, later.cost) {
            (Some(earlier), Some(later)) => later >= earlier,
            (Some(_), None) => false,
            (None, _) => true,
        }
}

fn progresses(events: &[AgentEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Progress(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Every conformance check, against one implementation.
///
/// `implementation` turns a script into the agent under test playing that
/// script — for [`Scripted`](crate::agent::Scripted) it is
/// [`Scripted::playing`](crate::agent::Scripted::playing) itself; a wrapper
/// around a real binary supplies a builder that stages the script as its
/// agent's feed.
///
/// # Panics
///
/// Panics — test-style, with the broken clause named — when the
/// implementation violates the contract.
pub fn conforms<A: CodingAgent>(implementation: impl Fn(Script) -> A) {
    stops_spent_when_tokens_run_out(&implementation);
    stops_spent_when_money_runs_out(&implementation);
    stalls_past_the_window_and_shrugs_off_shorter_silence(&implementation);
    emits_nothing_after_finished(&implementation);
    keeps_usage_monotone_over_a_backsliding_feed(&implementation);
    honors_the_stop_token(&implementation);
}

fn stops_spent_when_tokens_run_out<A: CodingAgent>(implementation: &impl Fn(Script) -> A) {
    let agent = implementation(vec![
        Play::Progress("working".to_owned()),
        Play::Usage(Usage::tokens(90, 20)),
        Play::Progress("smuggled past the ceiling".to_owned()),
        Play::Finish(Stop::Completed),
    ]);
    let over = task(Budget {
        max_tokens: Some(100),
        ..roomy()
    });
    let (events, stopped) = run(&agent, &over, &StopToken::new());
    assert_eq!(stopped, Stop::Spent, "110 tokens against a cap of 100");
    assert_eq!(
        progresses(&events),
        ["working"],
        "a spent run does no further work"
    );
}

fn stops_spent_when_money_runs_out<A: CodingAgent>(implementation: &impl Fn(Script) -> A) {
    let agent = implementation(vec![
        Play::Usage(Usage {
            cost: Some(Money {
                micro_usd: 2_000_000,
            }),
            ..Usage::tokens(10, 10)
        }),
        Play::Progress("smuggled past the ceiling".to_owned()),
        Play::Finish(Stop::Completed),
    ]);
    let over = task(Budget {
        max_cost: Some(Money {
            micro_usd: 1_000_000,
        }),
        ..roomy()
    });
    let (events, stopped) = run(&agent, &over, &StopToken::new());
    assert_eq!(stopped, Stop::Spent, "$2 reported against a cap of $1");
    assert!(
        progresses(&events).is_empty(),
        "a spent run does no further work"
    );
}

fn stalls_past_the_window_and_shrugs_off_shorter_silence<A: CodingAgent>(
    implementation: &impl Fn(Script) -> A,
) {
    let window = Duration::from_secs(30);
    let budget = Budget {
        stall: window,
        ..roomy()
    };

    let stalling = implementation(vec![
        Play::Progress("working".to_owned()),
        Play::Silence(window * 2),
        Play::Progress("back from the dead".to_owned()),
        Play::Finish(Stop::Completed),
    ]);
    let (events, stopped) = run(&stalling, &task(budget), &StopToken::new());
    assert_eq!(stopped, Stop::Stalled, "silence past the window is the end");
    assert_eq!(
        progresses(&events),
        ["working"],
        "nothing said after the window counts"
    );

    let pausing = implementation(vec![
        Play::Silence(window / 2),
        Play::Finish(Stop::Completed),
    ]);
    let (_, stopped) = run(&pausing, &task(budget), &StopToken::new());
    assert_eq!(
        stopped,
        Stop::Completed,
        "silence inside the window is just an agent thinking"
    );

    let accumulating = implementation(vec![
        Play::Silence(window * 2 / 3),
        Play::Silence(window * 2 / 3),
        Play::Finish(Stop::Completed),
    ]);
    let (_, stopped) = run(&accumulating, &task(budget), &StopToken::new());
    assert_eq!(
        stopped,
        Stop::Stalled,
        "gaps with no sign of life between them are one silence"
    );

    let resurfacing = implementation(vec![
        Play::Silence(window * 2 / 3),
        Play::Progress("still here".to_owned()),
        Play::Silence(window * 2 / 3),
        Play::Finish(Stop::Completed),
    ]);
    let (_, stopped) = run(&resurfacing, &task(budget), &StopToken::new());
    assert_eq!(
        stopped,
        Stop::Completed,
        "any beat is a sign of life, and resets the stall clock"
    );
}

fn emits_nothing_after_finished<A: CodingAgent>(implementation: &impl Fn(Script) -> A) {
    let agent = implementation(vec![
        Play::Progress("working".to_owned()),
        Play::Finish(Stop::Completed),
        Play::Progress("posthumous".to_owned()),
        Play::Usage(Usage::tokens(1_000, 1_000)),
        Play::Finish(Stop::Died {
            error: "second death".to_owned(),
        }),
    ]);
    // The terminal-Finished half is asserted inside `run` for every script;
    // this script is the one that earns it.
    let (_, stopped) = run(&agent, &task(roomy()), &StopToken::new());
    assert_eq!(
        stopped,
        Stop::Completed,
        "the first Finished is the run's end"
    );
}

fn keeps_usage_monotone_over_a_backsliding_feed<A: CodingAgent>(
    implementation: &impl Fn(Script) -> A,
) {
    let priced = |prompt, completion, micro_usd| Usage {
        cost: Some(Money { micro_usd }),
        ..Usage::tokens(prompt, completion)
    };
    let agent = implementation(vec![
        Play::Usage(priced(100, 10, 300)),
        // The feed runs its totals backward and forgets it ever named a
        // price; the wrapper's readings must do neither.
        Play::Usage(Usage::tokens(40, 4)),
        Play::Usage(priced(120, 12, 500)),
        Play::Finish(Stop::Completed),
    ]);
    // `run` asserts monotonicity on every emitted reading. How many
    // readings there are is the wrapper's business — suppressing a repeated
    // total conforms — so what is pinned here is the reading that matters:
    // the final one, authoritative by contract, must be the clamped maximum
    // of everything the feed claimed.
    let (events, stopped) = run(&agent, &task(roomy()), &StopToken::new());
    assert_eq!(stopped, Stop::Completed);
    let last = events.iter().rev().find_map(|event| match event {
        AgentEvent::Usage(usage) => Some(*usage),
        _ => None,
    });
    assert_eq!(
        last,
        Some(priced(120, 12, 500)),
        "the authoritative final reading is the high-water mark of the claims"
    );
}

fn honors_the_stop_token<A: CodingAgent>(implementation: &impl Fn(Script) -> A) {
    let agent = implementation(vec![
        Play::Progress("never happens".to_owned()),
        Play::Finish(Stop::Completed),
    ]);
    let stop = StopToken::new();
    stop.stop();
    let (events, stopped) = run(&agent, &task(roomy()), &stop);
    assert_eq!(stopped, Stop::Canceled, "a set token ends the run");
    assert!(
        progresses(&events).is_empty(),
        "a canceled run does no work on the way out"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Scripted;

    /// The acceptance gate: the scripted agent passes the same gauntlet
    /// every future implementation will.
    #[test]
    fn scripted_conforms() {
        conforms(Scripted::playing);
    }
}
