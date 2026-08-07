//! Talking to a language model.
//!
//! This is library infrastructure, not the window's conversation surface.
//! The same client will draft commit messages and summarize runs, and it is
//! what tests talk to; whatever the window's main pane eventually hosts is a
//! separate question.
//!
//! The shape mirrors the implementation side: a trait for the thing that
//! does the work ([`ChatModel`]), a vocabulary for what it is doing
//! ([`ChatEvent`](crate::event::ChatEvent)), and an owner of the truth
//! ([`Conversation`]).
//!
//! Everything here is blocking, on purpose. Async is the host's problem —
//! `spawn_blocking` under Tauri, plain threads in the daemon and in tests —
//! and cancellation is a [`StopToken`] checked between deltas, which is all
//! a Stop button will ever need.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::event::{ChatEvent, Remedy, Usage};
use crate::logging::Log;
use crate::tools::Registry;

mod scripted;
pub use scripted::Scripted;

// The HTTP client and its SSE framing are native-only: wasm32 builds of this
// crate exist to supply types to the frontend, not to make requests.
#[cfg(feature = "native")]
pub mod openai;
#[cfg(feature = "native")]
mod sse;

#[cfg(feature = "native")]
pub use openai::OpenAiCompatible;

/// One turn in a transcript, tagged on the wire by who said it.
///
/// The two tool-shaped turns are what make a tool loop possible over a
/// stateless protocol: the assistant's asks and the tools' answers live in
/// the transcript, because the transcript is the model's only memory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    /// What the assistant said, and any tools it asked for. Each field is
    /// omitted from the wire when empty, so a text-only turn serializes
    /// exactly as it did before tools existed.
    Assistant {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    /// One tool's answer, keyed back to the call that asked for it.
    Tool {
        tool_call_id: String,
        content: String,
    },
}

impl Message {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls: Vec::new(),
        }
    }

    /// The assistant turn that asked for tools — entered into the transcript
    /// so the model can see what it asked when the results come back.
    #[must_use]
    pub fn tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls,
        }
    }

    /// One tool's result, answering the call with `id`.
    #[must_use]
    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Tool {
            tool_call_id: id.into(),
            content: content.into(),
        }
    }
}

/// One call the assistant asked for. `id` is how the result finds its way
/// back to the ask.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: ToolKind,
    pub function: FunctionCall,
}

impl ToolCall {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: ToolKind::Function,
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

/// The kind of a tool or a tool call. The protocol defines exactly one, so
/// one variant is the whole type: a call of some other kind is
/// unrepresentable rather than checked for.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    #[default]
    Function,
}

/// A named function and its arguments — the model's half of one tool call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// The arguments as one JSON string, exactly as the model wrote them.
    /// Providers stream this in fragments; what lands here is the
    /// concatenation, and decoding it is the tool's business.
    pub arguments: String,
}

/// A function offered to the model: a name, what it does, and a JSON schema
/// for its parameters. The tool registry renders these; this module only
/// puts them on the wire.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    /// A JSON-schema object, carried opaquely.
    pub parameters: serde_json::Value,
}

/// Cancellation: a flag checked between deltas. Cloning shares the flag, so
/// a host can hand one copy to the thread that is streaming and keep another
/// for the button that stops it.
#[derive(Clone, Debug, Default)]
pub struct StopToken(Arc<AtomicBool>);

impl StopToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the turn in flight to wind up. Never unsets: a stopped turn
    /// stays stopped, and the next turn gets a fresh token.
    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// One assistant turn, assembled from its deltas.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reply {
    /// Every delta, concatenated.
    pub text: String,
    /// The calls the model asked for, when the turn ended asking for tools.
    /// Each one's argument fragments have been concatenated back into the
    /// one JSON string the model wrote.
    pub tool_calls: Vec<ToolCall>,
    /// Present only when the provider reported it.
    pub usage: Option<Usage>,
    /// The stop token fired before the model was finished.
    pub interrupted: bool,
}

/// A language model, as Epik needs one: a transcript in, one streamed reply
/// out.
///
/// The caller owns the history. That is the right seam for the stateless
/// chat-completions wire protocol, and for a session-owning model — a Claude
/// Code adapter, say — the answer is a narrower trait rather than a
/// contortion of this one.
pub trait ChatModel {
    /// Streams one assistant reply to `transcript`.
    ///
    /// Deltas go to `log` as they arrive; the assembled reply comes back.
    /// The call blocks. `stop` is checked between deltas, and when it is set
    /// the reply returns with what it has and `interrupted` set — an
    /// interruption is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error when the reply cannot be completed at all.
    fn reply(
        &mut self,
        transcript: &[Message],
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply>;
}

/// A model that authenticates with a key, and can be handed one later.
///
/// This is the seam that makes the paste-your-key card work: a key arriving
/// after the first turn was refused has to take effect on the conversation
/// already in progress, not on the next process. A model with nobody to
/// authenticate to implements this and does nothing with it.
pub trait Keyed {
    fn use_key(&mut self, key: Option<String>);
}

/// How many rounds of tool calls one send will run before refusing.
///
/// A model that answers every round of results with more calls is a loop
/// with someone else's bill; at the ceiling the turn ends in a
/// [`StillCalling`] refusal instead of another resend.
pub const MAX_TOOL_ROUNDS: usize = 8;

/// The refusal a turn ends in when the model is still calling tools at the
/// ceiling: a value a door renders, and what a caller downcasts out of the
/// turn's error to tell a runaway loop from a broken wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StillCalling {
    /// How many rounds of results the model got before the refusal.
    pub rounds: usize,
}

impl fmt::Display for StillCalling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the model was still calling tools after {} rounds of results",
            self.rounds
        )
    }
}

impl std::error::Error for StillCalling {}

/// Usage summed across rounds: reported when any round reported it, so a
/// multi-round turn bills for every request it made.
fn spend(total: Option<Usage>, round: Option<Usage>) -> Option<Usage> {
    match (total, round) {
        (Some(total), Some(round)) => Some(total.plus(round)),
        (total, round) => total.or(round),
    }
}

/// The canonical transcript, and the model that extends it.
///
/// A frontend's transcript is a *view*, folded from the event stream. This is
/// the truth. The duplication is deliberate: core owns what a conversation
/// is, so the daemon and every future client get it for free rather than
/// each reimplementing it.
#[derive(Debug)]
pub struct Conversation<M> {
    system: String,
    messages: Vec<Message>,
    model: M,
}

impl<M: ChatModel> Conversation<M> {
    pub fn new(system: impl Into<String>, model: M) -> Self {
        Self {
            system: system.into(),
            messages: Vec::new(),
            model,
        }
    }

    /// The system prompt followed by every completed turn: exactly what goes
    /// on the wire. The protocol is stateless, so this is resent every turn.
    #[must_use]
    pub fn transcript(&self) -> Vec<Message> {
        let mut transcript = Vec::with_capacity(self.messages.len() + 1);
        transcript.push(Message::system(self.system.clone()));
        transcript.extend(self.messages.iter().cloned());
        transcript
    }

    /// The completed turns, without the system prompt.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub const fn model(&self) -> &M {
        &self.model
    }

    /// The model, to be reconfigured mid-conversation — a key that has just
    /// arrived, say. The transcript is untouched, which is the whole reason
    /// this exists rather than building a second conversation: what was said
    /// stands.
    pub const fn model_mut(&mut self) -> &mut M {
        &mut self.model
    }

    /// Appends the user's turn, streams the assistant's, and finalizes it
    /// into the transcript.
    ///
    /// A turn that fails leaves no trace: the user's message is rolled back,
    /// so the transcript never holds a turn that produced nothing and a retry
    /// is a plain resend. An interrupted turn keeps whatever the model said,
    /// and says nothing when it said nothing.
    ///
    /// The failure is reported twice, deliberately: as a `Failed` event for
    /// whatever is watching the stream, and as an `Err` for the caller that
    /// has to decide what to do. A host uses the event and swallows the
    /// error, which is what keeps streamed failures and command errors on
    /// separate channels.
    ///
    /// # Errors
    ///
    /// Returns the model's error when the turn could not be completed.
    pub fn send(
        &mut self,
        text: impl Into<String>,
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply> {
        self.messages.push(Message::user(text));
        let transcript = self.transcript();
        match self.model.reply(&transcript, log, stop) {
            Ok(reply) => Ok(self.finish(reply, log)),
            Err(error) => {
                self.messages.pop();
                log.emit(ChatEvent::Failed {
                    error: format!("{error:#}"),
                });
                Err(error)
            }
        }
    }

    /// Appends the user's turn and runs the model against `tools` until it
    /// answers in text.
    ///
    /// A reply that ends asking for tools is one round: the ask and every
    /// answer are appended to the transcript and the grown transcript is
    /// resent — the transcript is the model's only memory, so this is how
    /// results reach it. A call the registry refuses — unknown tool,
    /// arguments that do not fit, the tool's own failure — answers with the
    /// refusal rendered as the result, because models recover well from
    /// failures they are told about. Tool activity narrates through `log` as
    /// it happens.
    ///
    /// The stop token is honored between calls and between rounds: an
    /// interrupted turn keeps whatever arrived so far, and a round cut short
    /// records only the calls that were answered, so the transcript stays
    /// coherent. As in [`send`](Self::send), an interruption is not an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns the model's error when a reply could not be completed at all
    /// — a broken wire fails the turn even mid-loop — and a [`StillCalling`]
    /// refusal when the model is still asking for tools after
    /// [`MAX_TOOL_ROUNDS`] rounds of results. A turn that failed before
    /// anything landed is rolled back like a failed [`send`](Self::send);
    /// one that failed after a round keeps the rounds that ran, because
    /// those calls really happened.
    pub fn send_with_tools(
        &mut self,
        text: impl Into<String>,
        tools: &mut Registry,
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply> {
        self.messages.push(Message::user(text));
        let sent = self.messages.len();
        let mut usage = None;
        let mut rounds = 0;
        loop {
            let transcript = self.transcript();
            let reply = match self.model.reply(&transcript, log, stop) {
                Ok(reply) => reply,
                Err(error) => {
                    if self.messages.len() == sent {
                        self.messages.pop();
                    }
                    log.emit(ChatEvent::Failed {
                        error: format!("{error:#}"),
                    });
                    return Err(error);
                }
            };
            usage = spend(usage, reply.usage);
            if reply.tool_calls.is_empty() || reply.interrupted {
                return Ok(self.finish(Reply { usage, ..reply }, log));
            }
            if rounds == MAX_TOOL_ROUNDS {
                let refusal = StillCalling { rounds };
                log.emit(ChatEvent::Failed {
                    error: refusal.to_string(),
                });
                return Err(refusal.into());
            }
            rounds += 1;
            self.round(&reply, tools, log, stop);
            // Between rounds is the other place a Stop button lands: a
            // stopped loop finalizes with what arrived instead of resending.
            if stop.is_stopped() {
                return Ok(self.finish(
                    Reply {
                        usage,
                        interrupted: true,
                        ..Reply::default()
                    },
                    log,
                ));
            }
        }
    }

    /// Runs one round of tool calls, entering the ask and each answer into
    /// the transcript. The stop token is checked between calls; a round cut
    /// short records only the calls that were answered, so the transcript
    /// stays coherent.
    fn round(
        &mut self,
        reply: &Reply,
        tools: &mut Registry,
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) {
        let mut answered = Vec::new();
        for call in &reply.tool_calls {
            if stop.is_stopped() {
                break;
            }
            log.emit(ChatEvent::ToolCallStarted {
                id: call.id.clone(),
                name: call.function.name.clone(),
                arguments: call.function.arguments.clone(),
            });
            let content = match tools.call(&call.function.name, &call.function.arguments) {
                Ok(value) => {
                    let content = value.to_string();
                    log.emit(ChatEvent::ToolCallFinished {
                        id: call.id.clone(),
                        content: content.clone(),
                    });
                    content
                }
                Err(refusal) => {
                    let content = refusal.to_string();
                    log.emit(ChatEvent::ToolCallRefused {
                        id: call.id.clone(),
                        error: content.clone(),
                        remedy: remedy(&refusal),
                    });
                    content
                }
            };
            answered.push((call.clone(), content));
        }
        if answered.is_empty() {
            // A stop before the first call: the ask never ran, so only the
            // text (when there is any) is worth remembering.
            if !reply.text.is_empty() {
                self.messages.push(Message::assistant(reply.text.clone()));
            }
        } else {
            let calls = answered.iter().map(|(call, _)| call.clone()).collect();
            self.messages
                .push(Message::tool_calls(reply.text.clone(), calls));
            for (call, content) in answered {
                self.messages.push(Message::tool_result(call.id, content));
            }
        }
    }

    /// Enters the reply's text into the transcript and closes the turn.
    fn finish(&mut self, reply: Reply, log: &mut dyn Log<ChatEvent>) -> Reply {
        if !reply.text.is_empty() {
            self.messages.push(Message::assistant(reply.text.clone()));
        }
        log.emit(ChatEvent::TurnFinished { usage: reply.usage });
        reply
    }
}

/// What a refusal invites the user to do. The one case the loop recognizes
/// is the GitHub client's want of a token, read out of the typed error the
/// registry preserved — the window answers it with the paste-your-PAT card.
#[cfg(feature = "native")]
fn remedy(refusal: &crate::tools::Error) -> Option<Remedy> {
    match refusal {
        crate::tools::Error::Failed { error, .. }
            if error.downcast_ref::<crate::github::Error>()
                == Some(&crate::github::Error::TokenAbsent) =>
        {
            Some(Remedy::GithubToken)
        }
        _ => None,
    }
}

/// On wasm this crate supplies types rather than tools, so no refusal here
/// ever has a remedy to name.
#[cfg(not(feature = "native"))]
const fn remedy(_: &crate::tools::Error) -> Option<Remedy> {
    None
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use serde_json::{Value, json};

    use super::*;
    use crate::logging::Silent;
    use crate::tools::Tool;

    const PERSONA: &str = "You are Epik.";

    /// Collects the event stream, and can pull the stop token mid-turn.
    #[derive(Debug, Default)]
    struct Watcher {
        events: Vec<ChatEvent>,
        stop_after_first_delta: Option<StopToken>,
    }

    impl Log<ChatEvent> for Watcher {
        fn emit(&mut self, event: ChatEvent) {
            if matches!(event, ChatEvent::Delta { .. })
                && let Some(stop) = self.stop_after_first_delta.take()
            {
                stop.stop();
            }
            self.events.push(event);
        }
    }

    fn deltas(events: &[ChatEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::Delta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_turn_streams_then_lands_in_the_transcript() {
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["Hello, ", "I'm Epik."]));
        let mut watcher = Watcher::default();

        let reply = chat
            .send("Hi", &mut watcher, &StopToken::new())
            .expect("the scripted turn succeeds");

        assert_eq!(reply.text, "Hello, I'm Epik.");
        assert_eq!(deltas(&watcher.events), "Hello, I'm Epik.");
        assert_eq!(
            watcher.events.last(),
            Some(&ChatEvent::TurnFinished { usage: None })
        );
        assert_eq!(
            chat.messages(),
            [Message::user("Hi"), Message::assistant("Hello, I'm Epik.")]
        );
    }

    #[test]
    fn the_system_prompt_heads_every_transcript_on_the_wire() {
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["one"]).then_saying(["two"]));

        chat.send("first", &mut Silent, &StopToken::new()).unwrap();
        chat.send("second", &mut Silent, &StopToken::new()).unwrap();

        let seen = chat.model().seen();
        assert_eq!(
            seen[0],
            [Message::system(PERSONA), Message::user("first")],
            "the first turn sends the persona and the question"
        );
        assert_eq!(
            seen[1],
            [
                Message::system(PERSONA),
                Message::user("first"),
                Message::assistant("one"),
                Message::user("second"),
            ],
            "the second turn resends the whole conversation"
        );
    }

    #[test]
    fn usage_reaches_the_finished_event_when_the_provider_reports_it() {
        let usage = Usage::tokens(11, 3);
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["hi"]).reporting(usage));
        let mut watcher = Watcher::default();

        chat.send("Hi", &mut watcher, &StopToken::new()).unwrap();

        assert_eq!(
            watcher.events.last(),
            Some(&ChatEvent::TurnFinished { usage: Some(usage) })
        );
    }

    #[test]
    fn a_failed_turn_leaves_the_transcript_as_it_was() {
        let mut chat = Conversation::new(PERSONA, Scripted::failing("the network went away"));
        let mut watcher = Watcher::default();

        let error = chat
            .send("Hi", &mut watcher, &StopToken::new())
            .expect_err("the scripted turn fails");

        assert!(error.to_string().contains("the network went away"));
        assert_eq!(
            watcher.events,
            [ChatEvent::Failed {
                error: "the network went away".to_owned()
            }]
        );
        assert!(
            chat.messages().is_empty(),
            "the user's rolled-back turn makes a retry a plain resend"
        );
    }

    #[test]
    fn a_later_turn_succeeds_after_a_failed_one() {
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::failing("timed out").then_saying(["second time lucky"]),
        );

        chat.send("Hi", &mut Silent, &StopToken::new())
            .expect_err("the first turn fails");
        chat.send("Hi", &mut Silent, &StopToken::new())
            .expect("the second turn succeeds");

        assert_eq!(
            chat.messages(),
            [Message::user("Hi"), Message::assistant("second time lucky"),],
            "the retry resent the same question and got an answer"
        );
    }

    #[test]
    fn stopping_mid_stream_finalizes_the_turn_with_what_arrived() {
        let stop = StopToken::new();
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["kept", "dropped", "dropped"]));
        let mut watcher = Watcher {
            stop_after_first_delta: Some(stop.clone()),
            ..Watcher::default()
        };

        let reply = chat
            .send("Hi", &mut watcher, &stop)
            .expect("an interruption is not an error");

        assert!(reply.interrupted);
        assert_eq!(reply.text, "kept");
        assert_eq!(
            watcher.events,
            [
                ChatEvent::Delta {
                    text: "kept".to_owned()
                },
                ChatEvent::TurnFinished { usage: None },
            ]
        );
        assert_eq!(
            chat.messages(),
            [Message::user("Hi"), Message::assistant("kept")],
            "a stopped turn is still a turn"
        );
    }

    #[test]
    fn stopping_before_the_first_delta_records_no_assistant_turn() {
        let stop = StopToken::new();
        stop.stop();
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["never sent"]));
        let mut watcher = Watcher::default();

        let reply = chat.send("Hi", &mut watcher, &stop).unwrap();

        assert!(reply.interrupted);
        assert!(reply.text.is_empty());
        assert_eq!(watcher.events, [ChatEvent::TurnFinished { usage: None }]);
        assert_eq!(
            chat.messages(),
            [Message::user("Hi")],
            "a model that said nothing gets no turn in the transcript"
        );
    }

    /// A tool with a failure vocabulary of its own, for the round-trip test.
    #[derive(Debug)]
    struct Grumpy;

    impl fmt::Display for Grumpy {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "the tool is grumpy today")
        }
    }

    impl std::error::Error for Grumpy {}

    /// A registry with one passthrough tool: `echo` answers with its own
    /// arguments, which makes every result easy to predict.
    fn echoing() -> Registry {
        let mut registry = Registry::new();
        registry.register(
            Tool::new(
                "echo",
                "Says the arguments back.",
                json!({ "type": "object" }),
            ),
            |args: Value| Ok::<_, Infallible>(args),
        );
        registry
    }

    /// A registry whose one tool pulls the stop token as it answers — the
    /// scripted stand-in for a Stop button pressed while a call runs.
    fn stopping(stop: &StopToken) -> Registry {
        let stopper = stop.clone();
        let mut registry = Registry::new();
        registry.register(
            Tool::new(
                "echo",
                "Says the arguments back.",
                json!({ "type": "object" }),
            ),
            move |args: Value| {
                stopper.stop();
                Ok::<_, Infallible>(args)
            },
        );
        registry
    }

    #[test]
    fn a_tool_round_runs_and_the_transcript_is_the_resent_history() {
        let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Let me check."])
                .asking([call.clone()])
                .then_saying(["The echo says hi."]),
        );

        let reply = chat
            .send_with_tools("Hi", &mut echoing(), &mut Silent, &StopToken::new())
            .expect("the two-round exchange completes");

        assert_eq!(reply.text, "The echo says hi.");
        let round = [
            Message::user("Hi"),
            Message::tool_calls("Let me check.", vec![call]),
            Message::tool_result("call-1", r#"{"text":"hi"}"#),
        ];
        let mut expected = round.to_vec();
        expected.push(Message::assistant("The echo says hi."));
        assert_eq!(chat.messages(), expected);

        let seen = chat.model().seen();
        assert_eq!(seen.len(), 2, "one round means exactly one resend");
        let mut resent = vec![Message::system(PERSONA)];
        resent.extend(round);
        assert_eq!(
            seen[1], resent,
            "the resent history is exactly the transcript"
        );
    }

    #[test]
    fn tool_activity_narrates_through_the_event_stream() {
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Let me check."])
                .asking([ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#)])
                .then_saying(["The echo says hi."]),
        );
        let mut watcher = Watcher::default();

        chat.send_with_tools("Hi", &mut echoing(), &mut watcher, &StopToken::new())
            .unwrap();

        assert_eq!(
            watcher.events,
            [
                ChatEvent::Delta {
                    text: "Let me check.".to_owned()
                },
                ChatEvent::ToolCallStarted {
                    id: "call-1".to_owned(),
                    name: "echo".to_owned(),
                    arguments: r#"{"text":"hi"}"#.to_owned(),
                },
                ChatEvent::ToolCallFinished {
                    id: "call-1".to_owned(),
                    content: r#"{"text":"hi"}"#.to_owned(),
                },
                ChatEvent::Delta {
                    text: "The echo says hi.".to_owned()
                },
                ChatEvent::TurnFinished { usage: None },
            ]
        );
    }

    #[test]
    fn a_handler_error_goes_back_to_the_model_as_data() {
        let mut tools = Registry::new();
        tools.register(
            Tool::new("grumpy", "Never cooperates.", json!({ "type": "object" })),
            |_: Value| Err::<Value, _>(Grumpy),
        );
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Trying."])
                .asking([ToolCall::new("call-1", "grumpy", "{}")])
                .then_saying(["It would not cooperate."]),
        );
        let mut watcher = Watcher::default();

        chat.send_with_tools("Go", &mut tools, &mut watcher, &StopToken::new())
            .expect("a told-about failure is not the turn's failure");

        let refusal = "grumpy: the tool is grumpy today";
        assert!(
            chat.model().seen()[1].contains(&Message::tool_result("call-1", refusal)),
            "the model reads the rendered refusal as a result"
        );
        assert!(
            watcher.events.contains(&ChatEvent::ToolCallRefused {
                id: "call-1".to_owned(),
                error: refusal.to_owned(),
                remedy: None,
            }),
            "the refusal narrates too, and grumpiness has no remedy"
        );
    }

    #[test]
    fn a_refusal_for_want_of_a_github_token_names_its_remedy() {
        let mut tools = Registry::new();
        tools.register(
            Tool::new("github_comment", "Comments.", json!({ "type": "object" })),
            |_: Value| Err::<Value, _>(crate::github::Error::TokenAbsent),
        );
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Commenting."])
                .asking([ToolCall::new("call-1", "github_comment", "{}")])
                .then_saying(["There is no token."]),
        );
        let mut watcher = Watcher::default();

        chat.send_with_tools("Comment", &mut tools, &mut watcher, &StopToken::new())
            .expect("a refused call is not the turn's failure");

        assert!(
            watcher.events.iter().any(|event| matches!(
                event,
                ChatEvent::ToolCallRefused {
                    remedy: Some(Remedy::GithubToken),
                    ..
                }
            )),
            "the typed refusal reaches the window as the case it is: {:?}",
            watcher.events
        );
    }

    #[test]
    fn an_unknown_tool_refuses_to_the_model_and_the_turn_goes_on() {
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Hmm."])
                .asking([ToolCall::new("call-9", "linear", "{}")])
                .then_saying(["No such tool here."]),
        );

        let reply = chat
            .send_with_tools("Go", &mut echoing(), &mut Silent, &StopToken::new())
            .expect("an unknown name refuses to the model, not to the caller");

        assert_eq!(reply.text, "No such tool here.");
        assert!(
            chat.model().seen()[1].contains(&Message::tool_result(
                "call-9",
                "no tool named linear is registered"
            )),
            "the registry's refusal reaches the model as data"
        );
    }

    #[test]
    fn a_wire_failure_mid_loop_fails_the_turn_but_keeps_the_round_that_ran() {
        let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Checking."])
                .asking([call.clone()])
                .then_failing("the network went away"),
        );
        let mut watcher = Watcher::default();

        let error = chat
            .send_with_tools("Hi", &mut echoing(), &mut watcher, &StopToken::new())
            .expect_err("a broken wire fails the turn even mid-loop");

        assert!(error.to_string().contains("the network went away"));
        assert_eq!(
            watcher.events.last(),
            Some(&ChatEvent::Failed {
                error: "the network went away".to_owned()
            })
        );
        assert_eq!(
            chat.messages(),
            [
                Message::user("Hi"),
                Message::tool_calls("Checking.", vec![call]),
                Message::tool_result("call-1", r#"{"text":"hi"}"#),
            ],
            "the round that ran really happened, so it stays"
        );
    }

    #[test]
    fn a_wire_failure_before_anything_landed_rolls_the_user_turn_back() {
        let mut chat = Conversation::new(PERSONA, Scripted::failing("refused at the door"));

        chat.send_with_tools("Hi", &mut echoing(), &mut Silent, &StopToken::new())
            .expect_err("the first reply failing fails the turn");

        assert!(
            chat.messages().is_empty(),
            "nothing landed, so a retry is a plain resend"
        );
    }

    #[test]
    fn the_rounds_ceiling_refuses_typed_rather_than_looping() {
        let mut script = Scripted::default();
        for round in 0..=MAX_TOOL_ROUNDS {
            script = script.then_saying(["Once more."]).asking([ToolCall::new(
                format!("call-{round}"),
                "echo",
                r#"{"n":1}"#,
            )]);
        }
        let mut chat = Conversation::new(PERSONA, script);
        let mut watcher = Watcher::default();

        let error = chat
            .send_with_tools("Go", &mut echoing(), &mut watcher, &StopToken::new())
            .expect_err("a model that never answers is refused");

        let refusal = error
            .downcast_ref::<StillCalling>()
            .expect("the refusal is typed, for a door to render");
        assert_eq!(
            refusal,
            &StillCalling {
                rounds: MAX_TOOL_ROUNDS
            }
        );
        assert_eq!(
            watcher.events.last(),
            Some(&ChatEvent::Failed {
                error: refusal.to_string()
            })
        );
        assert_eq!(
            chat.messages().len(),
            1 + 2 * MAX_TOOL_ROUNDS,
            "the rounds that ran stay; the unanswered ask does not"
        );
    }

    #[test]
    fn a_stop_between_rounds_keeps_what_arrived_and_does_not_resend() {
        let stop = StopToken::new();
        let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Checking."])
                .asking([call.clone()])
                .then_saying(["never sent"]),
        );

        let reply = chat
            .send_with_tools("Hi", &mut stopping(&stop), &mut Silent, &stop)
            .expect("an interruption is not an error");

        assert!(reply.interrupted);
        assert_eq!(
            chat.messages(),
            [
                Message::user("Hi"),
                Message::tool_calls("Checking.", vec![call]),
                Message::tool_result("call-1", r#"{"text":"hi"}"#),
            ],
            "the answered round stays, coherent for a later resend"
        );
        assert_eq!(chat.model().seen().len(), 1, "a stopped loop never resends");
    }

    #[test]
    fn a_stop_between_calls_records_only_the_answered_ask() {
        let stop = StopToken::new();
        let first = ToolCall::new("call-1", "echo", r#"{"a":1}"#);
        let second = ToolCall::new("call-2", "echo", r#"{"b":2}"#);
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Both, please."]).asking([first.clone(), second]),
        );

        let reply = chat
            .send_with_tools("Hi", &mut stopping(&stop), &mut Silent, &stop)
            .expect("an interruption is not an error");

        assert!(reply.interrupted);
        assert_eq!(
            chat.messages(),
            [
                Message::user("Hi"),
                Message::tool_calls("Both, please.", vec![first]),
                Message::tool_result("call-1", r#"{"a":1}"#),
            ],
            "the ask holds only the call that was answered"
        );
    }

    #[test]
    fn usage_sums_across_rounds() {
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::saying(["Checking."])
                .asking([ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#)])
                .reporting(Usage::tokens(10, 2))
                .then_saying(["Done."])
                .reporting(Usage::tokens(20, 3)),
        );
        let mut watcher = Watcher::default();

        chat.send_with_tools("Hi", &mut echoing(), &mut watcher, &StopToken::new())
            .unwrap();

        assert_eq!(
            watcher.events.last(),
            Some(&ChatEvent::TurnFinished {
                usage: Some(Usage::tokens(30, 5))
            }),
            "the whole turn bills for every request it made"
        );
    }
}
