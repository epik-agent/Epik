//! The zeroth [`ChatModel`]: no network, no keys, no nondeterminism.

use std::collections::VecDeque;

use anyhow::{Result, bail};

use crate::chat::{ChatModel, Message, Reply, StopToken};
use crate::event::{ChatEvent, Usage};
use crate::logging::Log;

/// A model that replies from a script.
///
/// It also keeps every transcript it was handed, which is how a test checks
/// what the library would have put on the wire without a wire.
#[derive(Debug, Default)]
pub struct Scripted {
    turns: VecDeque<Turn>,
    seen: Vec<Vec<Message>>,
}

#[derive(Debug)]
enum Turn {
    Reply {
        deltas: Vec<String>,
        usage: Option<Usage>,
    },
    Failure(String),
}

impl Scripted {
    /// One reply, arriving as the given deltas.
    #[must_use]
    pub fn saying<S: Into<String>>(deltas: impl IntoIterator<Item = S>) -> Self {
        Self::default().then_saying(deltas)
    }

    /// One turn that breaks.
    #[must_use]
    pub fn failing(error: impl Into<String>) -> Self {
        Self::default().then_failing(error)
    }

    /// Queues another reply behind the ones already scripted.
    #[must_use]
    pub fn then_saying<S: Into<String>>(mut self, deltas: impl IntoIterator<Item = S>) -> Self {
        self.turns.push_back(Turn::Reply {
            deltas: deltas.into_iter().map(Into::into).collect(),
            usage: None,
        });
        self
    }

    /// Queues a failure behind the turns already scripted.
    #[must_use]
    pub fn then_failing(mut self, error: impl Into<String>) -> Self {
        self.turns.push_back(Turn::Failure(error.into()));
        self
    }

    /// Makes the most recently queued reply report usage, as a provider that
    /// counts tokens would.
    #[must_use]
    pub fn reporting(mut self, reported: Usage) -> Self {
        if let Some(Turn::Reply { usage, .. }) = self.turns.back_mut() {
            *usage = Some(reported);
        }
        self
    }

    /// Every transcript this model was asked to reply to, in order.
    #[must_use]
    pub fn seen(&self) -> &[Vec<Message>] {
        &self.seen
    }
}

impl ChatModel for Scripted {
    fn reply(
        &mut self,
        transcript: &[Message],
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply> {
        self.seen.push(transcript.to_vec());
        let Some(turn) = self.turns.pop_front() else {
            bail!("the script has no turn left to play");
        };

        match turn {
            Turn::Failure(error) => bail!(error),
            Turn::Reply { deltas, usage } => {
                let mut reply = Reply::default();
                for delta in deltas {
                    // Between deltas is exactly where a real model can stop.
                    if stop.is_stopped() {
                        reply.interrupted = true;
                        return Ok(reply);
                    }
                    reply.text.push_str(&delta);
                    log.emit(ChatEvent::Delta { text: delta });
                }
                reply.usage = usage;
                Ok(reply)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::Silent;

    #[test]
    fn running_out_of_script_is_an_error_rather_than_silence() {
        let error = Scripted::default()
            .reply(&[], &mut Silent, &StopToken::new())
            .expect_err("an unscripted turn is a test bug, and should say so");
        assert!(error.to_string().contains("no turn left"));
    }
}
