//! Setting a coding agent to work.
//!
//! The [`ChatModel`](crate::chat::ChatModel) decomposition, repeated for
//! implementation: a vocabulary for what a run says ([`AgentEvent`]), one
//! trait with one verb ([`CodingAgent::run`]), and a scripted implementation
//! ([`Scripted`]) so scheduling, supervision, and the console are all
//! testable with no key, no network, and no external binary.
//!
//! Two properties are structural rather than promised. There is no `Ask`
//! variant: a wrapper has nowhere to put a question except
//! [`Stop::Blocked`], so autonomy is not a prompt's good behavior. And
//! governance — the budget, the stall window, the terminal `Finished` —
//! lives inside each implementation, because a black box mid-`run` cannot be
//! killed from outside; the trait therefore ships with a [`conformance`]
//! suite, and CI holds every implementation to it.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::chat::StopToken;
use crate::event::{Money, Usage};

pub mod conformance;
mod scripted;
pub use scripted::Scripted;

/// What the harness wants done. Steps are data, not methods: a new agent
/// implements one verb and inherits every kind, and a wrapper that
/// special-cases a kind internally is invisible to the harness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TaskKind {
    /// Implement an issue. The only kind so far; `Review`, `FixCi`, and
    /// their like arrive as variants, never as new traits.
    Implement,
}

/// One unit of work, fully provisioned before the agent sees it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Task {
    pub kind: TaskKind,
    /// Rendered by the harness from a template — issue, branches,
    /// conventions in, one prompt out.
    pub prompt: String,
    /// The agent's whole world on disk.
    pub worktree: PathBuf,
    /// Credentials injected, never discovered.
    pub env: Vec<(String, String)>,
    /// What the run may spend, and how long it may go quiet.
    pub budget: Budget,
}

/// What a run may spend before it is [`Stop::Spent`], and how long it may go
/// silent before it is [`Stop::Stalled`].
///
/// The ceilings mirror [`Usage`]'s denominations, each enforced only when
/// set. A ceiling in a denomination the configured agent never reports is a
/// launch-time configuration error — [`AgentError::BudgetUnenforceable`] —
/// never a silently unenforced number.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Budget {
    /// Ceiling on [total tokens](Usage::total_tokens): prompt, completion,
    /// and cache together.
    pub max_tokens: Option<u64>,
    /// Ceiling on reported cost.
    pub max_cost: Option<Money>,
    /// How long the agent may say nothing before the run is written off.
    pub stall: Duration,
}

impl Budget {
    /// The denominations this budget caps: what launch code checks against
    /// what the configured agent actually reports, so an unenforceable
    /// ceiling refuses at launch rather than lying for a whole run.
    pub fn denominations(self) -> impl Iterator<Item = Denomination> {
        self.max_tokens
            .map(|_| Denomination::Tokens)
            .into_iter()
            .chain(self.max_cost.map(|_| Denomination::Cost))
    }

    /// Whether `total` — a cumulative reading — exceeds any ceiling that is
    /// set. The moment this turns true, a conforming implementation stops
    /// with [`Stop::Spent`].
    #[must_use]
    pub fn spent(&self, total: &Usage) -> bool {
        self.max_tokens
            .is_some_and(|cap| total.total_tokens() > cap)
            || self
                .max_cost
                .is_some_and(|cap| total.cost.is_some_and(|cost| cost > cap))
    }
}

/// A unit a budget can be denominated in — and one an agent may never report.
///
/// Claude Code states its spend in dollars; Ollama honestly has no price,
/// and a price table maintained in Epik would go stale silently. So the
/// mismatch is a named refusal, not an estimate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Denomination {
    Tokens,
    Cost,
}

impl fmt::Display for Denomination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tokens => write!(f, "tokens"),
            Self::Cost => write!(f, "cost"),
        }
    }
}

/// One observable moment in an agent run.
///
/// A run is `Started`, then any mix of the middle three, then exactly one
/// `Finished` — after which a conforming implementation emits nothing,
/// whatever the process under it does. Serde types and nothing else, like
/// every vocabulary in [`crate::event`]: these cross the same boundaries.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AgentEvent {
    /// The wrapper names what it is running. Always first.
    Started { agent: String, version: String },
    /// Narrative, for the console.
    Progress(String),
    /// Cumulative totals, monotone nondecreasing; the last reading before
    /// [`Finished`](Self::Finished) is authoritative. The budget watches
    /// this.
    Usage(Usage),
    /// Opaque and agent-flavored, rendered best-effort.
    Detail(serde_json::Value),
    /// The run is over. Terminal by contract, not convention — the
    /// [`conformance`] suite asserts it.
    Finished(Stop),
}

/// How a run ended — never whether the work is done. Done is the harness's
/// judgment, made out-of-band through GitHub: the PR exists, CI is green.
/// Self-reported success is not in the vocabulary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Stop {
    /// The agent believes it finished. A belief, not a fact.
    Completed,
    /// The dignified exit: the agent could not proceed, and says why. There
    /// is no `Ask` variant — this is where a question would have gone.
    Blocked { report: String },
    /// The budget ran out.
    Spent,
    /// Silent past the stall window.
    Stalled,
    /// The stop token was honored, or the process killed.
    Canceled,
    /// The process ended without finishing.
    Died { error: String },
}

/// Why a run could not be conducted at all.
///
/// An agent that started and came to grief is a [`Stop`]; this is the
/// wrapper's own failure — refusals as vocabulary, in the house rule, never
/// prose scraped from a log.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentError {
    /// The budget caps a denomination this agent never reports. A
    /// configuration error caught at launch, before a worktree exists —
    /// the check lives with launch code; this is the shape it refuses in.
    BudgetUnenforceable { denomination: Denomination },
    /// The wrapper itself broke — not the agent dying, which is
    /// [`Stop::Died`], but the run being unconductable. For [`Scripted`],
    /// a script with no finish left to play.
    Broken { error: String },
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetUnenforceable { denomination } => write!(
                f,
                "the budget caps {denomination}, which this agent never reports"
            ),
            Self::Broken { error } => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AgentError {}

/// A coding agent, as Epik needs one: a provisioned task in, a stream of
/// events out, one verb.
pub trait CodingAgent {
    /// Runs `task` to its stop.
    ///
    /// The call blocks for the whole run — one thread per running agent,
    /// the host's own pattern — and events reach `sink` as they happen.
    /// `stop` is the same cancellation chat honors: checked between
    /// observable moments and answered with [`Stop::Canceled`], because a
    /// run cut short is an outcome, not an error. The returned [`Stop`]
    /// always agrees with the one in the final [`AgentEvent::Finished`].
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] only when no run could be conducted at all;
    /// everything that happens to a running agent is a [`Stop`].
    fn run(
        &self,
        task: &Task,
        sink: &mut dyn FnMut(AgentEvent),
        stop: &StopToken,
    ) -> Result<Stop, AgentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_events_round_trip_through_json() {
        let events = [
            AgentEvent::Started {
                agent: "scripted".to_owned(),
                version: "0.1.0".to_owned(),
            },
            AgentEvent::Progress("compiling".to_owned()),
            AgentEvent::Usage(Usage::tokens(18, 6)),
            AgentEvent::Detail(serde_json::json!({ "subagent": "reviewer" })),
            AgentEvent::Finished(Stop::Blocked {
                report: "the issue contradicts itself".to_owned(),
            }),
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: AgentEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, back);
        }
    }

    #[test]
    fn a_budget_names_the_denominations_it_caps() {
        let budget = Budget {
            max_tokens: Some(1_000),
            max_cost: None,
            stall: Duration::from_mins(1),
        };
        assert_eq!(
            budget.denominations().collect::<Vec<_>>(),
            [Denomination::Tokens],
            "what launch code holds against the agent's reported denominations"
        );
        let error = AgentError::BudgetUnenforceable {
            denomination: Denomination::Cost,
        };
        assert_eq!(
            error.to_string(),
            "the budget caps cost, which this agent never reports"
        );
    }

    #[test]
    fn a_budget_is_spent_only_in_denominations_it_caps() {
        let budget = Budget {
            max_tokens: Some(100),
            max_cost: None,
            stall: Duration::from_mins(1),
        };
        assert!(!budget.spent(&Usage::tokens(90, 10)), "the ceiling itself");
        assert!(budget.spent(&Usage::tokens(90, 11)), "one token over");
        let expensive = Usage {
            cost: Some(Money {
                micro_usd: u64::MAX,
            }),
            ..Usage::tokens(1, 1)
        };
        assert!(
            !budget.spent(&expensive),
            "an uncapped denomination has no ceiling to exceed"
        );
    }
}
