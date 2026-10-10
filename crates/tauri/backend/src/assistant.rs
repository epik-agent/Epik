//! The assistant round-trip. Commands carry intent in; events carry
//! observation out.
//!
//! Every item goes out through [`AssistantState::emit`], which is the
//! whole channel in one place: under one lock it keeps the item in the
//! canonical [`Snapshot`] when the item is conversation, returns the
//! turn when the item is the one that ends it, stamps the item with its
//! place as an [`Event`] and sends it. [`get_transcript`] hands out that
//! snapshot under the same lock, so its position and the events' places
//! agree exactly: a window compares the two and counts each event once
//! or not at all.
//!
//! [`send_message`] emits the user's message and runs the library's
//! tool loop on its own thread — deltas stream out as `AssistantDelta`
//! events, each dispatched tool lands as `ToolCall` and `ToolResult`
//! items the moment it happens, and the completed reply goes out as a
//! `Message`. A turn that fails emits `TurnFailed` instead; the window
//! shows nothing it didn't receive as an event. One turn in flight at a
//! time: a send during a turn is refused on the command channel,
//! because a misplaced intent is not an assistant failure.
//!
//! A turn can also ask. A tool handler runs on the turn's thread, which
//! is allowed to block, so a question suspends the turn until the user
//! answers: [`ask`] registers a pending entry, emits `Question`, and
//! blocks on a channel; [`answer_question`] — the command the window's
//! card sends — finds the entry and sends the answer in; the asker then
//! emits `QuestionResolved`, which the channel keeps like every other
//! settled item, and hands the answer back to the handler. The window
//! never shows a resolution it did not receive as an event.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, PoisonError};

use epik_core::assistant::{
    Answer, Ask, Client, Error, Event, Refusal, Role, SYSTEM_PROMPT, Snapshot, TRANSCRIPT_EVENT,
    TranscriptItem, run,
};
use epik_core::config::Config;
use epik_core::github::GitHub;
use epik_core::keystore::Resolved;
use epik_core::model_tools::{self, Registry, Tool};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::secrets::{KeyStore, OsKeyring};

/// The keystore entry the turn's key comes from.
pub(crate) const API_KEY_NAME: &str = "ANTHROPIC_API_KEY";

/// The keystore entry the GitHub verbs authenticate with. Optional: absent
/// means public reads still work and the writing verbs refuse per-call.
pub(crate) const GITHUB_TOKEN_NAME: &str = "GITHUB_TOKEN";

/// Where to send someone whose turn failed for want of a key.
const SET_KEY_HINT: &str = "Set the Anthropic key in Settings (Cmd+,).";

/// The assistant's whole state: the channel as it stands, the way out
/// of it, and the questions a turn is waiting on.
pub struct AssistantState {
    /// Under the one lock that keeps its position and the events' places
    /// in step.
    channel: Mutex<Snapshot>,
    /// Where every event goes: the window, or a test's receiver.
    send: Box<dyn Fn(Event) + Send + Sync>,
    /// Question id to the sender the blocked handler is waiting on.
    pending: Mutex<BTreeMap<String, Sender<Answer>>>,
    /// The monotonic source of question ids.
    next_question: AtomicU64,
}

impl AssistantState {
    /// A state whose events go out through `send`.
    pub fn new(send: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self {
            channel: Mutex::default(),
            send: Box::new(send),
            pending: Mutex::default(),
            next_question: AtomicU64::default(),
        }
    }

    fn channel(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.channel.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the turn and posts the user's message, in one: the
    /// transcript the turn runs over, or `None` when the turn already
    /// belongs to one in flight, which keeps it until the item that ends
    /// it goes out.
    fn post(&self, text: String) -> Option<Vec<TranscriptItem>> {
        let mut channel = self.channel();
        if std::mem::replace(&mut channel.turning, true) {
            return None;
        }
        Self::send(
            &mut channel,
            &self.send,
            TranscriptItem::Message {
                role: Role::User,
                text,
            },
        );
        Some(channel.items.clone())
    }

    /// Sends `item` out, stamped with its place on the channel. An item
    /// that is conversation is kept in the snapshot, and the item that
    /// ends the turn returns the claim — all under the one lock, held
    /// for the send and released before anything else, so a snapshot
    /// taken between two events is exact.
    fn emit(&self, item: TranscriptItem) {
        Self::send(&mut self.channel(), &self.send, item);
    }

    /// [`emit`](Self::emit) on a channel already held.
    fn send(channel: &mut Snapshot, send: &dyn Fn(Event), item: TranscriptItem) {
        if item.is_conversation() {
            channel.items.push(item.clone());
        }
        if item.ends_turn() {
            channel.turning = false;
        }
        let event = Event {
            seq: channel.seq,
            item,
        };
        channel.seq += 1;
        send(event);
    }

    /// The channel as it stands.
    fn snapshot(&self) -> Snapshot {
        self.channel().clone()
    }

    /// Registers a fresh pending question and returns its id with the
    /// receiver the asker will block on.
    fn open_question(&self) -> (String, std::sync::mpsc::Receiver<Answer>) {
        let id = self
            .next_question
            .fetch_add(1, Ordering::SeqCst)
            .to_string();
        let (sender, receiver) = channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), sender);
        (id, receiver)
    }

    /// Takes the pending entry for `id`, if there is one.
    fn close_question(&self, id: &str) -> Option<Sender<Answer>> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
    }
}

/// Asks the user `ask` and blocks until they answer. Emits `Question`
/// on the way out; on the way back emits `QuestionResolved` — the
/// record the window and the conversation both keep — and returns the
/// answer to the handler. A question whose entry is dropped without an
/// answer resolves as declined: the persona should stop and wait, which
/// is what a decline says.
fn ask(state: &AssistantState, ask: Ask) -> Answer {
    let (id, receiver) = state.open_question();
    state.emit(TranscriptItem::Question {
        id: id.clone(),
        ask: ask.clone(),
    });
    let answer = receiver.recv().unwrap_or(Answer::Declined);
    state.emit(TranscriptItem::QuestionResolved {
        id,
        ask,
        answer: answer.clone(),
    });
    answer
}

/// Delivers `answer` to the handler waiting on question `id`. An id with
/// nothing pending — a stale card, a double click — is refused in words.
fn answer(state: &AssistantState, id: &str, answer: Answer) -> Result<(), String> {
    let sender = state
        .close_question(id)
        .ok_or_else(|| format!("no question with id {id} is waiting for an answer"))?;
    sender
        .send(answer)
        .map_err(|_| format!("the turn that asked question {id} is no longer waiting"))
}

/// The tool through which the persona asks where a repository should
/// live. `asker` is the whole modality: it takes the ask and comes back
/// with the answer, however long that takes.
fn choose_repository(asker: impl Fn(Ask) -> Answer + 'static) -> Tool {
    Tool::new(
        "choose_repository",
        "Asks the user, in the window, where a repository should live, and waits for the          answer: a local path or git URL. Use this only when the conversation has not already          supplied a repository location — a user who typed a path has already answered. The          result is either {\"url\": ...} or {\"declined\": true}; a decline means the user          would rather not say — stop and wait for them, do not ask again.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Your own wording of the question, as the user will see it.",
                },
            },
        }),
        Box::new(move |arguments| {
            let prompt = arguments["prompt"]
                .as_str()
                .filter(|prompt| !prompt.trim().is_empty())
                .unwrap_or("Where should this repository live?")
                .to_owned();
            Ok(match asker(Ask::Repository { prompt }) {
                Answer::Repository { url } => serde_json::json!({ "url": url }),
                // A check answer never comes off a repository card; if
                // one ever did, it is no location, which is a decline.
                Answer::Check { .. } | Answer::Declined => {
                    serde_json::json!({ "declined": true })
                }
            })
        }),
    )
}

/// What answers a [`turn`]: handed the delta and record sinks, it comes
/// back with the model's final text, or why there is none.
trait Replier:
    FnOnce(&mut dyn FnMut(&str), &mut dyn FnMut(TranscriptItem)) -> Result<String, Error>
{
}

impl<F> Replier for F where
    F: FnOnce(&mut dyn FnMut(&str), &mut dyn FnMut(TranscriptItem)) -> Result<String, Error>
{
}

/// The replier every real turn uses: the library loop over `registry`,
/// with every tool call and its result recorded as they happen.
fn replying_with<'a>(
    client: &'a Client,
    system: &'a str,
    transcript: &'a [TranscriptItem],
    registry: &'a Registry,
    stop: &'a AtomicBool,
) -> impl Replier + 'a {
    move |on_delta, record| {
        run(
            client,
            system,
            transcript,
            registry,
            on_delta,
            |call, result| {
                record(TranscriptItem::tool_call(&call.name, &call.arguments));
                record(TranscriptItem::tool_result(&call.name, result));
            },
            stop,
        )
    }
}

/// One turn against any replier, emitting what happens in the order it
/// happens: deltas while the model speaks, tool calls and their results
/// as the replier reports them, then the completed message, or
/// `TurnFailed` with the reason. Generic over the replier and the
/// effect, which is what makes it testable without a wire or a window.
fn turn(reply: impl Replier, emit: impl Fn(TranscriptItem)) {
    let mut on_delta = |delta: &str| {
        emit(TranscriptItem::AssistantDelta {
            text: delta.to_owned(),
        });
    };
    let mut record = |item| emit(item);
    match reply(&mut on_delta, &mut record) {
        Ok(text) => emit(TranscriptItem::Message {
            role: Role::Assistant,
            text,
        }),
        Err(error) => emit(TranscriptItem::TurnFailed {
            reason: error.to_string(),
        }),
    }
}

/// Posts the user's message, then holds the turn: the reply streams back
/// as transcript events while this command has long since returned.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AssistantState>,
    text: String,
) -> Result<(), Refusal> {
    let Some(transcript) = state.post(text) else {
        return Err(Refusal::TurnInFlight);
    };

    let key = match OsKeyring.resolve(API_KEY_NAME) {
        Resolved::Found(key) => key,
        Resolved::Absent => {
            state.emit(TranscriptItem::TurnFailed {
                reason: format!("No Anthropic key is set. {SET_KEY_HINT}"),
            });
            return Ok(());
        }
        Resolved::Unreachable(reason) => {
            state.emit(TranscriptItem::TurnFailed {
                reason: format!(
                    "The system keychain couldn't be reached ({reason}). {SET_KEY_HINT}"
                ),
            });
            return Ok(());
        }
    };

    // The GitHub token is optional; a keystore that has none — or cannot
    // be reached for it — just means an unauthenticated client, whose
    // writing verbs refuse individually with their own message.
    let github_token = match OsKeyring.resolve(GITHUB_TOKEN_NAME) {
        Resolved::Found(token) => Some(token),
        Resolved::Absent | Resolved::Unreachable(_) => None,
    };

    // What the configuration states now — the file at startup, or the
    // last write from the settings window. Cloned out of managed state
    // so the thread owns it.
    let config = app
        .state::<Mutex<Config>>()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();

    // The turn gets its own thread: an inline turn would hold this async
    // context — and the window's patience — for its whole duration.
    std::thread::spawn(move || {
        // The same key rides into a build agent's environment; the CLI's
        // own logged-in auth would also do, but the one the user set here
        // is the one they mean.
        let agent_key = Some(key.clone());
        let client = Client::anthropic(key, config.model.chat);
        // Assembled fresh each turn, so a token pasted mid-session
        // reaches the very next turn.
        let mut registry = Registry::standard();
        registry.extend(model_tools::github::all(
            GitHub::new(github_token.clone()),
            config.github.owner.clone(),
        ));
        // The one question rail: the persona asks through
        // choose_repository, and the turn's thread blocks until the
        // window answers.
        let asker = |app: AppHandle| move |question| ask(&app.state::<AssistantState>(), question);
        registry.extend(crate::build::feature_tools(
            &app,
            agent_key,
            config.model.agent,
            github_token,
            config.github.owner,
        ));
        registry.register(choose_repository(asker(app.clone())));
        let stop = AtomicBool::new(false);
        let state = app.state::<AssistantState>();
        turn(
            replying_with(&client, &SYSTEM_PROMPT, &transcript, &registry, &stop),
            |item| state.emit(item),
        );
    });
    Ok(())
}

/// The window's answer to a pending question. Refused in words when no
/// question with that id is waiting.
#[tauri::command]
pub async fn answer_question(
    state: State<'_, AssistantState>,
    id: String,
    answer: Answer,
) -> Result<(), String> {
    self::answer(&state, &id, answer)
}

/// The channel as it stands, for a window that has just opened its
/// eyes: the conversation so far, the place the next event takes, and
/// whether a turn is in flight.
#[tauri::command]
pub async fn get_transcript(state: State<'_, AssistantState>) -> Result<Snapshot, String> {
    Ok(state.snapshot())
}

/// The state behind `app`'s window: every event goes out on
/// [`TRANSCRIPT_EVENT`].
pub fn state(app: AppHandle) -> AssistantState {
    AssistantState::new(move |event| {
        let _ = app.emit(TRANSCRIPT_EVENT, event);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::sync::mpsc::Receiver;

    fn said(role: Role, text: &str) -> TranscriptItem {
        TranscriptItem::Message {
            role,
            text: text.to_owned(),
        }
    }

    /// An observer that keeps what it sees, for `turn`'s emit.
    fn into(sink: &RefCell<Vec<TranscriptItem>>) -> impl Fn(TranscriptItem) + '_ {
        |item| sink.borrow_mut().push(item)
    }

    /// A state whose events land on the returned receiver.
    fn state() -> (Arc<AssistantState>, Receiver<Event>) {
        let (events_in, events) = channel();
        let state = AssistantState::new(move |event| {
            let _ = events_in.send(event);
        });
        (Arc::new(state), events)
    }

    fn delta(text: &str) -> TranscriptItem {
        TranscriptItem::AssistantDelta {
            text: text.to_owned(),
        }
    }

    /// Each item's kind in a word — the order of a turn's events, read
    /// without their content.
    fn kinds(items: &[TranscriptItem]) -> Vec<&'static str> {
        items
            .iter()
            .map(|item| match item {
                TranscriptItem::Question { .. } => "question",
                TranscriptItem::QuestionResolved { .. } => "resolved",
                TranscriptItem::ToolCall { .. } => "call",
                TranscriptItem::ToolResult { ok: true, .. } => "ok",
                TranscriptItem::AssistantDelta { .. } => "delta",
                TranscriptItem::Message { .. } => "message",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn every_event_goes_out_stamped_with_its_place_from_zero() {
        let (state, events) = state();
        state.emit(said(Role::User, "hello"));
        state.emit(delta("Hel"));
        state.emit(said(Role::Assistant, "Hello"));
        let stamped: Vec<_> = events.try_iter().collect();
        assert_eq!(
            stamped,
            [
                Event {
                    seq: 0,
                    item: said(Role::User, "hello")
                },
                Event {
                    seq: 1,
                    item: delta("Hel")
                },
                Event {
                    seq: 2,
                    item: said(Role::Assistant, "Hello")
                },
            ]
        );
    }

    #[test]
    fn the_channel_keeps_what_is_conversation_and_sends_everything() {
        let (state, events) = state();
        let call = TranscriptItem::ToolCall {
            name: "current_time".to_owned(),
            arguments: "{}".to_owned(),
        };
        let failed = TranscriptItem::TurnFailed {
            reason: "no key".to_owned(),
        };
        state.emit(said(Role::User, "hello"));
        state.emit(delta("Hel"));
        state.emit(call.clone());
        state.emit(failed.clone());

        let sent: Vec<_> = events.try_iter().map(|event| event.item).collect();
        assert_eq!(
            sent,
            [
                said(Role::User, "hello"),
                delta("Hel"),
                call.clone(),
                failed
            ]
        );
        assert_eq!(state.snapshot().items, [said(Role::User, "hello"), call]);
    }

    /// The snapshot stands between two events: everything before its
    /// position is in it or was news of the moment, and the next event
    /// out takes exactly that position.
    #[test]
    fn a_snapshot_is_positioned_at_the_next_event() {
        let (state, events) = state();
        assert_eq!(state.snapshot(), Snapshot::default());
        state.emit(said(Role::User, "hello"));
        state.emit(delta("Hel"));

        let snapshot = state.snapshot();
        assert_eq!(snapshot.items, [said(Role::User, "hello")]);
        assert_eq!(snapshot.seq, 2);

        state.emit(said(Role::Assistant, "Hello"));
        assert_eq!(events.try_iter().last().unwrap().seq, snapshot.seq);
    }

    #[test]
    fn a_post_claims_the_turn_until_the_item_that_ends_it_goes_out() {
        let (state, events) = state();
        state.emit(said(Role::Assistant, "earlier"));
        assert_eq!(
            state.post("hello".to_owned()),
            Some(vec![
                said(Role::Assistant, "earlier"),
                said(Role::User, "hello")
            ]),
            "the transcript the turn runs over"
        );
        assert_eq!(
            events.try_iter().last().unwrap().item,
            said(Role::User, "hello"),
            "the message went out"
        );
        assert_eq!(
            state.post("again".to_owned()),
            None,
            "a second claim is refused"
        );
        assert!(state.snapshot().turning);

        state.emit(delta("Hel"));
        assert!(state.snapshot().turning, "a turn in progress");
        state.emit(said(Role::Assistant, "Hello"));
        assert!(!state.snapshot().turning);
        assert!(
            state.post("again".to_owned()).is_some(),
            "a returned claim can be taken again"
        );

        state.emit(TranscriptItem::TurnFailed {
            reason: "no key".to_owned(),
        });
        assert!(!state.snapshot().turning, "a failure ends the turn too");
    }

    #[test]
    fn a_turn_streams_deltas_then_emits_the_reply() {
        let emitted = RefCell::new(Vec::new());

        turn(
            |on_delta, _| {
                on_delta("Hel");
                on_delta("lo");
                Ok("Hello".to_owned())
            },
            into(&emitted),
        );

        assert_eq!(
            *emitted.borrow(),
            [delta("Hel"), delta("lo"), said(Role::Assistant, "Hello")]
        );
    }

    #[test]
    fn a_failed_turn_emits_the_reason() {
        let emitted = RefCell::new(Vec::new());

        turn(
            |_, _| Err(Error::Api("Your credit balance is too low.".to_owned())),
            into(&emitted),
        );

        assert_eq!(
            *emitted.borrow(),
            [TranscriptItem::TurnFailed {
                reason: "Your credit balance is too low.".to_owned()
            }]
        );
    }

    #[test]
    fn recorded_tool_items_are_emitted_as_they_happen() {
        let emitted = RefCell::new(Vec::new());
        let call = TranscriptItem::ToolCall {
            name: "current_time".to_owned(),
            arguments: "{}".to_owned(),
        };
        let result = TranscriptItem::ToolResult {
            name: "current_time".to_owned(),
            ok: true,
            content: "{\"local\":\"noon\"}".to_owned(),
        };

        turn(
            |_, record| {
                record(call.clone());
                record(result.clone());
                Ok("It's noon.".to_owned())
            },
            into(&emitted),
        );

        assert_eq!(
            *emitted.borrow(),
            [call, result, said(Role::Assistant, "It's noon.")]
        );
    }

    /// The whole backend wiring against the scripted model: the library
    /// loop drives a real client, and the turn observes it into items.
    #[test]
    fn a_scripted_tool_turn_flows_through_the_turn_as_items() {
        use epik_core::assistant::scripted::{Fragment, ScriptedModel, Turn as Script};

        let model = ScriptedModel::spawn(vec![
            Script::ToolCalls(vec![vec![Fragment::open(
                0,
                "call_1",
                "current_time",
                "{}",
            )]]),
            Script::text(&["It's ", "noon."]),
        ]);
        let client = Client::new(model.base_url(), "scripted".to_owned(), None);
        let registry = Registry::standard();
        let stop = AtomicBool::new(false);
        let emitted = RefCell::new(Vec::new());

        turn(
            replying_with(
                &client,
                "system",
                &[said(Role::User, "what time is it?")],
                &registry,
                &stop,
            ),
            into(&emitted),
        );

        assert_eq!(
            kinds(&emitted.borrow()),
            ["call", "ok", "delta", "delta", "message"]
        );
        assert_eq!(
            model.requests()[1]["messages"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["tool_call_id"],
            "call_1"
        );
    }

    fn where_to() -> Ask {
        Ask::Repository {
            prompt: "Where should this project live?".to_owned(),
        }
    }

    /// The whole ask round trip: the asker blocks on its own thread
    /// (the turn's, in the app), the Question goes out as an event, the
    /// answer arrives through the command's inner function, and the
    /// resolution is kept and emitted before the handler gets its
    /// answer back.
    #[test]
    fn an_ask_emits_the_question_and_resolves_when_answered() {
        let (state, events) = state();
        let asker = std::thread::spawn({
            let state = Arc::clone(&state);
            move || ask(&state, where_to())
        });

        let TranscriptItem::Question { id, ask: asked } = events.recv().unwrap().item else {
            panic!("the first event is the question");
        };
        assert_eq!(asked, where_to());
        assert!(
            state.snapshot().items.is_empty(),
            "a pending question stays on the channel"
        );

        answer(
            &state,
            &id,
            Answer::Repository {
                url: "/tmp/wumpus.git".to_owned(),
            },
        )
        .unwrap();

        let answered = asker.join().unwrap();
        assert_eq!(
            answered,
            Answer::Repository {
                url: "/tmp/wumpus.git".to_owned()
            }
        );
        let resolved = TranscriptItem::QuestionResolved {
            id,
            ask: where_to(),
            answer: answered,
        };
        assert_eq!(
            events.recv().unwrap().item,
            resolved,
            "the resolution is emitted"
        );
        assert_eq!(state.snapshot().items, [resolved], "and kept");
        assert!(state.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn an_answer_to_no_pending_question_is_refused_in_words() {
        let (state, _events) = state();
        let error = answer(&state, "42", Answer::Declined).unwrap_err();
        assert!(error.contains("42"), "{error}");
        assert!(error.contains("no question"), "{error}");
    }

    #[test]
    fn a_question_answered_twice_is_refused_the_second_time() {
        let (state, _events) = state();
        let (id, receiver) = state.open_question();
        answer(&state, &id, Answer::Declined).unwrap();
        assert_eq!(receiver.recv().unwrap(), Answer::Declined);
        assert!(answer(&state, &id, Answer::Declined).is_err());
    }

    #[test]
    fn question_ids_are_distinct_and_monotonic() {
        let (state, _events) = state();
        let (first, _) = state.open_question();
        let (second, _) = state.open_question();
        assert!(first.parse::<u64>().unwrap() < second.parse::<u64>().unwrap());
    }

    /// The scripted model calls choose_repository; the test plays the
    /// user, answering the pending question; the model's next request
    /// carries the url as the tool's result. The one place in this crate
    /// where a turn is suspended on a question and resumed.
    #[test]
    fn a_scripted_choose_repository_turn_suspends_until_the_user_answers() {
        use epik_core::assistant::scripted::{Fragment, ScriptedModel, Turn as Script};

        let model = ScriptedModel::spawn(vec![
            Script::ToolCalls(vec![vec![Fragment::open(
                0,
                "call_1",
                "choose_repository",
                r#"{"prompt":"Where should this project live?"}"#,
            )]]),
            Script::text(&["Making it there."]),
        ]);
        let base_url = model.base_url();
        let (state, events) = state();

        let turning = std::thread::spawn({
            let state = Arc::clone(&state);
            move || {
                let client = Client::new(base_url, "scripted".to_owned(), None);
                let mut registry = Registry::default();
                registry.register(choose_repository({
                    let state = Arc::clone(&state);
                    move |question| ask(&state, question)
                }));
                turn(
                    replying_with(
                        &client,
                        "system",
                        &[said(Role::User, "write me hunt the wumpus")],
                        &registry,
                        &AtomicBool::new(false),
                    ),
                    |item| state.emit(item),
                );
            }
        });

        let TranscriptItem::Question { id, ask: asked } = events.recv().unwrap().item else {
            panic!("the turn asks first");
        };
        assert_eq!(asked, where_to());
        answer(
            &state,
            &id,
            Answer::Repository {
                url: "/tmp/wumpus.git".to_owned(),
            },
        )
        .unwrap();
        turning.join().unwrap();

        let events: Vec<_> = std::iter::once(TranscriptItem::Question {
            id: id.clone(),
            ask: asked,
        })
        .chain(events.try_iter().map(|event| event.item))
        .collect();
        assert_eq!(
            kinds(&events),
            ["question", "resolved", "call", "ok", "delta", "message"]
        );

        let second = &model.requests()[1];
        let result = second["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(result["role"], "tool");
        assert_eq!(result["tool_call_id"], "call_1");
        assert_eq!(
            result["content"],
            serde_json::json!({ "url": "/tmp/wumpus.git" }).to_string()
        );
    }

    #[test]
    fn a_declined_question_is_an_ok_result_the_model_reads() {
        let tool = choose_repository(|_| Answer::Declined);
        let mut registry = Registry::default();
        registry.register(tool);
        assert_eq!(
            registry.dispatch("choose_repository", "{}"),
            Ok(serde_json::json!({ "declined": true }))
        );
    }

    #[test]
    fn a_missing_prompt_gets_a_default_wording() {
        let mut registry = Registry::default();
        registry.register(choose_repository(|question| {
            let Ask::Repository { prompt } = question else {
                panic!("a repository card asks for a repository");
            };
            assert!(!prompt.is_empty());
            Answer::Repository { url: prompt }
        }));
        let url = registry.dispatch("choose_repository", "{}").unwrap()["url"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(url.contains("live"), "{url}");
    }
}
