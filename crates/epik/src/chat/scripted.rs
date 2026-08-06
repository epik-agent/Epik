//! The zeroth [`ChatModel`]: no network, no keys, no nondeterminism.

use std::collections::VecDeque;

use anyhow::{Result, bail};

use crate::chat::{ChatModel, Keyed, Message, Reply, StopToken, ToolCall};
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
    key: Option<String>,
}

#[derive(Debug)]
enum Turn {
    Reply {
        deltas: Vec<String>,
        tool_calls: Vec<ToolCall>,
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
            tool_calls: Vec::new(),
            usage: None,
        });
        self
    }

    /// Makes the most recently queued reply end by asking for `calls`, as a
    /// model that wants tools would.
    #[must_use]
    pub fn asking(mut self, calls: impl IntoIterator<Item = ToolCall>) -> Self {
        if let Some(Turn::Reply { tool_calls, .. }) = self.turns.back_mut() {
            *tool_calls = calls.into_iter().collect();
        }
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

    /// The key it was last handed. A scripted model has nobody to
    /// authenticate to, so this is here to be asserted on: it is how a test
    /// sees that a key stored through the paste-your-key card reached the
    /// model rather than only the store.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }
}

impl Keyed for Scripted {
    fn use_key(&mut self, key: Option<String>) {
        self.key = key;
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
            Turn::Reply {
                deltas,
                tool_calls,
                usage,
            } => {
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
                // An interrupted reply carries no calls: on the wire the
                // asks arrive at the end, so a cut-off turn never asked.
                reply.tool_calls = tool_calls;
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
