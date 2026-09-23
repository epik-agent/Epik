//! The wire shapes of the OpenAI-compatible chat-completions protocol,
//! and the pure mappings in and out of them.

use super::{Error, Role, TranscriptItem};
use crate::model_tools::{ToolCall, ToolSpec};

/// One message as the request will carry it. The transcript's completed
/// messages map to [`Text`]; the other two forms exist only inside a tool
/// turn, where the loop appends what it did so the model can go on.
///
/// [`Text`]: Message::Text
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Message {
    /// An utterance, from the transcript.
    Text { role: Role, text: String },
    /// The assistant's turn that was tool calls instead of words.
    ToolCalls(Vec<ToolCall>),
    /// A tool's answer, addressed to the call by its id. `content` is the
    /// whole answer — the wire never truncates.
    ToolResult { id: String, content: String },
}

impl Message {
    /// The transcript's completed messages, in order, as wire messages.
    /// Deltas and failures are not conversation; tool items are the
    /// transcript's record of past turns, already summarized by the
    /// assistant text that followed them, and carry no call ids — so
    /// none of them go to the wire.
    pub(super) fn from_transcript(transcript: &[TranscriptItem]) -> Vec<Self> {
        transcript
            .iter()
            .filter_map(|item| match item {
                TranscriptItem::Message { role, text } => Some(Self::Text {
                    role: *role,
                    text: text.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

/// Assembles server-sent-event lines into event data payloads.
///
/// Fed one line at a time: `data:` lines accumulate, a blank line
/// completes the event and hands back its payload (multiple data lines
/// joined with newlines, per the SSE contract), and every other field —
/// event names, ids, comments — is ignored, which is all the protocol
/// this stream speaks.
#[derive(Debug, Default)]
pub(super) struct Events {
    data: Vec<String>,
}

impl Events {
    pub(super) fn line(&mut self, line: &str) -> Option<String> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.flush();
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
        }
        None
    }

    /// The event in progress, for a stream that ended without its blank
    /// line.
    pub(super) fn flush(&mut self) -> Option<String> {
        if self.data.is_empty() {
            None
        } else {
            let payload = self.data.join("\n");
            self.data.clear();
            Some(payload)
        }
    }
}

/// What one SSE data payload means to the stream.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Step {
    /// A fragment of the reply.
    Delta(String),
    /// Fragments of tool calls, to be accumulated by index.
    ToolCalls(Vec<ToolCallFragment>),
    /// The model finished by asking for tools instead of speaking.
    ToolsFinished,
    /// The end of the stream.
    Done,
    /// Protocol housekeeping with nothing to say — a role announcement,
    /// an empty choice.
    Skip,
}

#[derive(serde::Serialize)]
pub(super) struct Request<'a> {
    model: &'a str,
    messages: Vec<Wire<'a>>,
    stream: bool,
    // Serialized only when present, so a plain chat request's JSON
    // stays byte-identical to the shape before tools existed.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    tools: &'a [ToolSpec],
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
}

/// A [`Message`] as the request body spells it.
#[derive(serde::Serialize)]
struct Wire<'a> {
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<&'a [ToolCall]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

impl<'a> Wire<'a> {
    fn text(role: &'a str, content: &'a str) -> Self {
        Self {
            role,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

const fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// The request body: the system message first, then `messages` in
/// order — utterances, the assistant's tool-call turns, and tools'
/// answers addressed by call id.
pub(super) fn request<'a>(
    model: &'a str,
    system: &'a str,
    messages: &'a [Message],
    tools: &'a [ToolSpec],
    tool_choice: Option<&'a str>,
) -> Request<'a> {
    let mut wire = vec![Wire::text("system", system)];
    wire.extend(messages.iter().map(|message| match message {
        Message::Text { role, text } => Wire::text(role_name(*role), text),
        Message::ToolCalls(calls) => Wire {
            role: "assistant",
            content: None,
            tool_calls: Some(calls),
            tool_call_id: None,
        },
        Message::ToolResult { id, content } => Wire {
            role: "tool",
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(id),
        },
    }));
    Request {
        model,
        messages: wire,
        stream: true,
        tools,
        tool_choice,
    }
}

#[derive(serde::Deserialize)]
struct Chunk {
    choices: Vec<Choice>,
}

#[derive(serde::Deserialize)]
struct Choice {
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallFragment>>,
}

/// One delta's worth of one tool call: the id and name arrive once
/// per index, the arguments in as many pieces as the stream likes.
#[derive(Debug, Eq, PartialEq, serde::Deserialize)]
pub(super) struct ToolCallFragment {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionFragment>,
}

#[derive(Debug, Eq, PartialEq, serde::Deserialize)]
struct FunctionFragment {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Assembles [`ToolCallFragment`]s into completed calls, keyed by
/// index. Yields them in index order when the stream finishes.
#[derive(Debug, Default)]
pub(super) struct Calls(std::collections::BTreeMap<usize, ToolCall>);

impl Calls {
    pub(super) fn absorb(&mut self, fragments: Vec<ToolCallFragment>) {
        for fragment in fragments {
            let call = self.0.entry(fragment.index).or_insert_with(|| ToolCall {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
            if let Some(id) = fragment.id {
                call.id = id;
            }
            if let Some(function) = fragment.function {
                if let Some(name) = function.name {
                    call.name = name;
                }
                if let Some(arguments) = function.arguments {
                    call.arguments.push_str(&arguments);
                }
            }
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn finish(self) -> Vec<ToolCall> {
        self.0.into_values().collect()
    }
}

/// Reads one data payload of the stream.
pub(super) fn step(payload: &str) -> Result<Step, Error> {
    if payload.trim() == "[DONE]" {
        return Ok(Step::Done);
    }
    let chunk: Chunk =
        serde_json::from_str(payload).map_err(|_| Error::MalformedStream(payload.to_owned()))?;
    let Some(choice) = chunk.choices.into_iter().next() else {
        return Ok(Step::Skip);
    };
    if let Some(fragments) = choice.delta.tool_calls {
        return Ok(Step::ToolCalls(fragments));
    }
    if let Some(content) = choice.delta.content {
        return Ok(Step::Delta(content));
    }
    if choice.finish_reason.as_deref() == Some("tool_calls") {
        return Ok(Step::ToolsFinished);
    }
    Ok(Step::Skip)
}

#[derive(serde::Deserialize)]
struct ApiError {
    error: ApiErrorBody,
}

#[derive(serde::Deserialize)]
struct ApiErrorBody {
    message: String,
}

/// The API's own words for what went wrong, verbatim when the body
/// carries them, with the raw body and then the bare status as
/// fallbacks.
pub(super) fn api_message(status: &str, body: &str) -> String {
    if let Ok(error) = serde_json::from_str::<ApiError>(body) {
        return error.error.message;
    }
    if body.trim().is_empty() {
        status.to_owned()
    } else {
        body.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(role: Role, text: &str) -> Message {
        Message::Text {
            role,
            text: text.to_owned(),
        }
    }

    #[test]
    fn sse_events_assemble_line_by_line() {
        let mut events = Events::default();
        assert_eq!(events.line("data: one"), None);
        assert_eq!(events.line(""), Some("one".to_owned()));
        assert_eq!(events.line("data: two"), None);
        assert_eq!(events.line(""), Some("two".to_owned()));
    }

    #[test]
    fn multi_line_data_joins_with_newlines() {
        let mut events = Events::default();
        events.line("data: first");
        events.line("data: second");
        assert_eq!(events.line(""), Some("first\nsecond".to_owned()));
    }

    #[test]
    fn other_sse_fields_comments_and_crlf_are_taken_in_stride() {
        let mut events = Events::default();
        events.line("event: message\r");
        events.line(": a comment");
        events.line("id: 7");
        events.line("data: payload\r");
        assert_eq!(events.line("\r"), Some("payload".to_owned()));
    }

    #[test]
    fn a_blank_line_with_no_data_is_no_event() {
        let mut events = Events::default();
        assert_eq!(events.line(""), None);
    }

    #[test]
    fn an_unterminated_final_event_still_flushes() {
        let mut events = Events::default();
        events.line("data: tail");
        assert_eq!(events.flush(), Some("tail".to_owned()));
        assert_eq!(events.flush(), None);
    }

    #[test]
    fn the_request_leads_with_the_system_message_and_streams() {
        let messages = [text(Role::User, "hi"), text(Role::Assistant, "hi yourself")];
        let body = serde_json::to_value(request("m", "be brief", &messages, &[], None)).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "model": "m",
                "messages": [
                    { "role": "system", "content": "be brief" },
                    { "role": "user", "content": "hi" },
                    { "role": "assistant", "content": "hi yourself" },
                ],
                "stream": true,
            })
        );
    }

    /// The byte-identity contract: a request with no tools serializes
    /// to exactly the JSON this client sent before tools existed.
    #[test]
    fn a_plain_request_is_byte_identical_to_the_pre_tools_shape() {
        let messages = [text(Role::User, "hi")];
        let body = serde_json::to_string(&request("m", "s", &messages, &[], None)).unwrap();
        assert_eq!(
            body,
            r#"{"model":"m","messages":[{"role":"system","content":"s"},{"role":"user","content":"hi"}],"stream":true}"#
        );
    }

    #[test]
    fn tools_and_tool_choice_ride_along_when_present() {
        let tools = [ToolSpec::function(
            "current_time".to_owned(),
            "the local time".to_owned(),
            serde_json::json!({ "type": "object", "properties": {} }),
        )];
        let body = serde_json::to_value(request("m", "s", &[], &tools, Some("auto"))).unwrap();
        assert_eq!(
            body["tools"],
            serde_json::json!([{
                "type": "function",
                "function": {
                    "name": "current_time",
                    "description": "the local time",
                    "parameters": { "type": "object", "properties": {} },
                },
            }])
        );
        assert_eq!(body["tool_choice"], serde_json::json!("auto"));
    }

    #[test]
    fn a_tool_turn_maps_to_an_assistant_call_and_an_addressed_answer() {
        let messages = [
            text(Role::User, "what time is it?"),
            Message::ToolCalls(vec![ToolCall {
                id: "call_1".to_owned(),
                name: "current_time".to_owned(),
                arguments: "{}".to_owned(),
            }]),
            Message::ToolResult {
                id: "call_1".to_owned(),
                content: "{\"local\":\"noon\"}".to_owned(),
            },
        ];
        let body = serde_json::to_value(request("m", "s", &messages, &[], None)).unwrap();
        assert_eq!(
            body["messages"],
            serde_json::json!([
                { "role": "system", "content": "s" },
                { "role": "user", "content": "what time is it?" },
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "current_time", "arguments": "{}" },
                    }],
                },
                {
                    "role": "tool",
                    "content": "{\"local\":\"noon\"}",
                    "tool_call_id": "call_1",
                },
            ])
        );
    }

    #[test]
    fn only_completed_messages_map_from_the_transcript() {
        let transcript = [
            TranscriptItem::AssistantDelta {
                text: "he".to_owned(),
            },
            TranscriptItem::TurnFailed {
                reason: "x".to_owned(),
            },
            TranscriptItem::ToolCall {
                name: "t".to_owned(),
                arguments: "{}".to_owned(),
            },
            TranscriptItem::ToolResult {
                name: "t".to_owned(),
                ok: true,
                content: "1".to_owned(),
            },
            TranscriptItem::Message {
                role: Role::User,
                text: "hi".to_owned(),
            },
        ];
        assert_eq!(
            Message::from_transcript(&transcript),
            [text(Role::User, "hi")]
        );
    }

    #[test]
    fn a_content_delta_steps_forward() {
        let step = step(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#).unwrap();
        assert_eq!(step, Step::Delta("Hel".to_owned()));
    }

    #[test]
    fn a_tool_call_delta_steps_into_fragments() {
        let step = step(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"f","arguments":"{\"a\""}}]}}]}"#,
        )
        .unwrap();
        assert!(
            matches!(step, Step::ToolCalls(ref f) if f.len() == 1),
            "{step:?}"
        );
    }

    #[test]
    fn finish_reason_tool_calls_ends_the_speaking() {
        let step = step(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert_eq!(step, Step::ToolsFinished);
    }

    /// The calls a stream of tool-call `payloads` assembles into.
    fn assembled(payloads: &[&str]) -> Vec<ToolCall> {
        let mut calls = Calls::default();
        for payload in payloads {
            let Step::ToolCalls(fragments) = step(payload).unwrap() else {
                panic!("expected fragments");
            };
            calls.absorb(fragments);
        }
        calls.finish()
    }

    /// The id and name arrive once per index; the arguments string
    /// accumulates across as many fragments as the stream likes.
    #[test]
    fn fragmented_arguments_accumulate_into_one_call() {
        let calls = assembled(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"f","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"a\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}"#,
        ]);
        assert_eq!(
            calls,
            [ToolCall {
                id: "call_1".to_owned(),
                name: "f".to_owned(),
                arguments: "{\"a\":1}".to_owned(),
            }]
        );
    }

    /// Two calls whose fragments interleave still come out whole, in
    /// index order.
    #[test]
    fn interleaved_indexes_assemble_into_ordered_calls() {
        let calls = assembled(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"beta","arguments":"{\"b\""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"alpha","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":":2}"}}]}}]}"#,
        ]);
        assert_eq!(
            calls,
            [
                ToolCall {
                    id: "call_a".to_owned(),
                    name: "alpha".to_owned(),
                    arguments: "{}".to_owned(),
                },
                ToolCall {
                    id: "call_b".to_owned(),
                    name: "beta".to_owned(),
                    arguments: "{\"b\":2}".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_role_announcement_is_housekeeping() {
        let step = step(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#).unwrap();
        assert_eq!(step, Step::Skip);
    }

    #[test]
    fn done_ends_the_stream() {
        assert_eq!(step("[DONE]").unwrap(), Step::Done);
    }

    #[test]
    fn gibberish_is_a_malformed_stream_not_a_panic() {
        let stepped = step("not json");
        assert!(
            matches!(stepped, Err(Error::MalformedStream(_))),
            "{stepped:?}"
        );
    }

    #[test]
    fn the_apis_own_error_message_survives_verbatim() {
        let body = r#"{"error":{"message":"Your credit balance is too low.","type":"invalid_request_error"}}"#;
        assert_eq!(
            api_message("400 Bad Request", body),
            "Your credit balance is too low."
        );
        assert_eq!(api_message("500", "backend melted"), "backend melted");
        assert_eq!(api_message("502 Bad Gateway", "  "), "502 Bad Gateway");
    }
}
