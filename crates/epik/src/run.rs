//! An issue run's own vocabulary: the state machine narrating itself.
//!
//! [`RunEvent`] is the second vocabulary on the envelope channel, as the
//! walking-skeleton ADR promised: the same
//! [`Envelope`](crate::event::Envelope) and the same sinks carry it, so a
//! console folds a run the way the chat pane folds chat — one more match
//! arm, no new plumbing. State transitions are the harness's own narration;
//! the active agent's [`AgentEvent`] stream rides within. Serde types and
//! nothing else up here, which keeps the vocabulary wasm-clean; the machine
//! that emits it — [`IssueRun`] — is native.

use serde::{Deserialize, Serialize};

use crate::agent::AgentEvent;

#[cfg(feature = "native")]
mod issue;
#[cfg(feature = "native")]
pub use issue::{Concluded, Evidence, IssueRun};

/// The states of an issue run, in the order a clean run enters them: the
/// old `epik:issue` loop as code structure.
///
/// In v1 the run is wide — one `TaskKind::Implement` invocation does
/// everything from implementing through closing, prompt-instructed — and
/// the middle states verify rather than act. They exist from day one so
/// each future taper moves work across a boundary that is already drawn,
/// never the architecture.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Phase {
    /// The harness fetches the clone and hangs the worktree off it:
    /// `Task.worktree` is harness input, never the agent's errand.
    Worktree,
    /// The wide invocation: the agent implements, opens the pull request,
    /// watches it, reviews, merges, and closes — prompt-instructed, for
    /// now.
    Implement,
    /// Verifies the pull request exists and its checks are green.
    Watch,
    /// The review boundary. Nothing to verify in v1 — the review happened
    /// inside [`Implement`](Self::Implement) — but the state is where the
    /// review taper will land.
    Review,
    /// Verifies the pull request merged into the target branch.
    Merge,
    /// Verifies the issue is closed.
    Close,
    /// The retention policy applied: the worktree is removed on a verified
    /// success, kept for forensics otherwise.
    Cleanup,
}

/// One observable moment in an issue run.
///
/// A run is a walk through [`Phase`]s, each entry narrated, the active
/// agent's whole stream carried within, ended by exactly one
/// [`Finished`](Self::Finished) bearing the harness's verdict.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunEvent {
    /// The run enters a state — the harness's narration, never the agent's.
    Entered(Phase),
    /// The active agent speaks.
    Agent(AgentEvent),
    /// The run is over. Terminal.
    Finished(Verdict),
}

/// How the run ended, as the harness judged it — never as the agent
/// believed.
///
/// Done is verified out-of-band through GitHub after the agent stops; a
/// `Stop::Completed` that GitHub disbelieves is a failed run with a report,
/// not a success.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Verdict {
    /// GitHub agrees: the pull request merged into the target branch, its
    /// checks green, the issue closed.
    Done,
    /// Anything less, with the report to render.
    Failed { report: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{ConversationId, Envelope};

    #[test]
    fn run_events_round_trip_through_json() {
        let events = [
            RunEvent::Entered(Phase::Worktree),
            RunEvent::Agent(AgentEvent::Progress("compiling".to_owned())),
            RunEvent::Entered(Phase::Cleanup),
            RunEvent::Finished(Verdict::Failed {
                report: "issue #7 is still open".to_owned(),
            }),
            RunEvent::Finished(Verdict::Done),
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: RunEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, back);
        }
    }

    #[test]
    fn the_envelope_channel_carries_the_run_vocabulary_as_promised() {
        let envelope = Envelope {
            conversation: ConversationId(3),
            event: RunEvent::Entered(Phase::Implement),
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let back: Envelope<RunEvent> = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope, back, "the second vocabulary, same channel");
    }
}
