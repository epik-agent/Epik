//! The zeroth [`CodingAgent`]: no key, no network, no external binary.

use std::time::Duration;

use crate::agent::{AgentError, AgentEvent, CodingAgent, Play, Script, Stop, Task};
use crate::chat::StopToken;
use crate::event::Usage;

/// An agent that plays back a script.
///
/// The script is the raw feed a real wrapper would read off a process —
/// including feeds no well-behaved process would produce, which is the
/// point. Governance is not skipped for being fake: `Scripted` enforces the
/// budget, the stall window, and the terminal `Finished` exactly as every
/// wrapper must, so the [conformance suite](crate::agent::conformance)
/// holds it to the same contract the real thing will answer to.
#[derive(Debug, Default)]
pub struct Scripted {
    plays: Script,
}

impl Scripted {
    /// An agent that will play back `script`. The signature the conformance
    /// suite wants: `conforms(Scripted::playing)`.
    #[must_use]
    pub const fn playing(script: Script) -> Self {
        Self { plays: script }
    }
}

impl CodingAgent for Scripted {
    fn run(
        &self,
        task: &Task,
        sink: &mut dyn FnMut(AgentEvent),
        stop: &StopToken,
    ) -> Result<Stop, AgentError> {
        sink(AgentEvent::Started {
            agent: "scripted".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        // Governance state: the wrapper's own totals, fed by the script's
        // claims but never run backward by them, and the stall clock —
        // quiet accumulated since the last sign of life, as a reader
        // blocking in `recv_timeout` would accumulate it. Simulated, so the
        // suite runs in microseconds.
        let mut total = Usage::default();
        let mut quiet = Duration::ZERO;
        for play in &self.plays {
            // Between beats is exactly where a real wrapper can notice the
            // token: its reader thread wakes per line or per timeout.
            if stop.is_stopped() {
                return Ok(finish(sink, Stop::Canceled));
            }
            match play {
                Play::Progress(text) => {
                    quiet = Duration::ZERO;
                    sink(AgentEvent::Progress(text.clone()));
                }
                Play::Detail(value) => {
                    quiet = Duration::ZERO;
                    sink(AgentEvent::Detail(value.clone()));
                }
                Play::Silence(gap) => {
                    quiet = quiet.saturating_add(*gap);
                    if quiet >= task.budget.stall {
                        return Ok(finish(sink, Stop::Stalled));
                    }
                }
                Play::Usage(claimed) => {
                    quiet = Duration::ZERO;
                    total = total.max(*claimed);
                    sink(AgentEvent::Usage(total));
                    if task.budget.spent(&total) {
                        return Ok(finish(sink, Stop::Spent));
                    }
                }
                Play::Finish(stop) => return Ok(finish(sink, stop.clone())),
            }
        }
        // Mirrors the chat Scripted: running out of script is a test bug,
        // and should say so rather than impersonate a death.
        Err(AgentError::Broken {
            error: "the script ended without finishing".to_owned(),
        })
    }
}

/// The one way out: every exit emits `Finished` and returns the same stop,
/// which is how "nothing after `Finished`" holds by construction.
fn finish(sink: &mut dyn FnMut(AgentEvent), stop: Stop) -> Stop {
    sink(AgentEvent::Finished(stop.clone()));
    stop
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::agent::{Budget, TaskKind};

    fn task() -> Task {
        Task {
            kind: TaskKind::Implement,
            prompt: "implement issue #0".to_owned(),
            worktree: PathBuf::from("."),
            env: Vec::new(),
            budget: Budget {
                max_tokens: None,
                max_cost: None,
                stall: Duration::from_mins(1),
            },
        }
    }

    fn run(agent: &Scripted) -> (Vec<AgentEvent>, Result<Stop, AgentError>) {
        let mut events = Vec::new();
        let mut sink = |event: AgentEvent| events.push(event);
        let result = agent.run(&task(), &mut sink, &StopToken::new());
        (events, result)
    }

    #[test]
    fn a_benign_script_plays_the_whole_event_path() {
        let agent = Scripted::playing(vec![
            Play::Progress("reading the issue".to_owned()),
            Play::Usage(Usage::tokens(18, 6)),
            Play::Detail(json!({ "tool": "cargo test" })),
            Play::Finish(Stop::Completed),
        ]);

        let (events, result) = run(&agent);

        assert_eq!(result, Ok(Stop::Completed));
        assert_eq!(
            events,
            [
                AgentEvent::Started {
                    agent: "scripted".to_owned(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                },
                AgentEvent::Progress("reading the issue".to_owned()),
                AgentEvent::Usage(Usage::tokens(18, 6)),
                AgentEvent::Detail(json!({ "tool": "cargo test" })),
                AgentEvent::Finished(Stop::Completed),
            ]
        );
    }

    #[test]
    fn a_blocked_agent_carries_its_report_out() {
        let agent = Scripted::playing(vec![Play::Finish(Stop::Blocked {
            report: "the issue asks for two contradictory things".to_owned(),
        })]);

        let (events, result) = run(&agent);

        let stop = Stop::Blocked {
            report: "the issue asks for two contradictory things".to_owned(),
        };
        assert_eq!(result, Ok(stop.clone()));
        assert_eq!(events.last(), Some(&AgentEvent::Finished(stop)));
    }

    #[test]
    fn running_out_of_script_is_an_error_rather_than_a_death() {
        let agent = Scripted::default();

        let (events, result) = run(&agent);

        assert_eq!(
            result,
            Err(AgentError::Broken {
                error: "the script ended without finishing".to_owned()
            }),
            "an unscripted end is a test bug, and should say so"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Finished(_))),
            "a run that errored never finished"
        );
    }
}
