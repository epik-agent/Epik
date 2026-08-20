//! The chat round-trip. Commands carry intent in; events carry
//! observation out.
//!
//! [`send_message`] appends the user's message to the canonical
//! [`Conversation`], emits it back on the transcript channel, and runs
//! the library's tool loop on its own thread — deltas stream out as
//! `AssistantDelta` events, each dispatched tool lands as `ToolCall` and
//! `ToolResult` items the moment it happens, and the completed reply is
//! appended and emitted as a `Message`. A turn that fails emits
//! `TurnFailed` instead; the window shows nothing it didn't receive as
//! an event. One turn in
//! flight at a time: a send during a turn is refused on the command
//! channel, because a misplaced intent is not a chat failure.
//!
//! A turn can also ask. A tool handler runs on the turn's thread, which
//! is allowed to block, so a question suspends the turn until the user
//! answers: [`ask`] registers a pending entry, emits `Question`, and
//! blocks on a channel; [`answer_question`] — the command the window's
//! card sends — finds the entry and sends the answer in; the asker then
//! appends and emits `QuestionResolved`, the same append-and-emit pair as
//! every other item, and hands the answer back to the handler. The
//! window never shows a resolution it did not receive as an event.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, PoisonError};

use epik::chat::{
    Answer, Ask, ChatError, Client, Conversation, Role, SYSTEM_PROMPT, TRANSCRIPT_EVENT,
    TranscriptItem,
};
use epik::github::GitHub;
use epik::keystore::{KeyStore, OsKeyring, Resolved};
use epik::tools::{self, Registry, Tool};
use tauri::{AppHandle, Emitter, Manager, State};

/// The keystore entry the turn's key comes from.
const API_KEY_NAME: &str = "ANTHROPIC_API_KEY";

/// The keystore entry the GitHub verbs authenticate with. Optional: absent
/// means public reads still work and the writing verbs refuse per-call.
const GITHUB_TOKEN_NAME: &str = "GITHUB_TOKEN";

/// Where to send someone whose turn failed for want of a key.
const SET_KEY_HINT: &str = "Set the Anthropic key in Settings (Cmd+,).";

/// The chat's whole state: the canonical transcript, whether a turn is
/// in flight, and the questions a turn is waiting on.
#[derive(Default)]
pub struct ChatState {
    conversation: Mutex<Conversation>,
    turning: AtomicBool,
    /// Question id to the sender the blocked handler is waiting on.
    pending: Mutex<BTreeMap<String, Sender<Answer>>>,
    /// The monotonic source of question ids.
    next_question: AtomicU64,
}

impl ChatState {
    /// Claims the turn: true when it is now this caller's, false when it
    /// already belongs to a turn in flight, which keeps it.
    fn begin_turn(&self) -> bool {
        !self.turning.swap(true, Ordering::SeqCst)
    }

    /// Returns the claim.
    fn end_turn(&self) {
        self.turning.store(false, Ordering::SeqCst);
    }

    /// Appends `item` to the canonical conversation.
    fn append(&self, item: &TranscriptItem) {
        self.conversation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(item.clone());
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
/// on the way out; on the way back appends and emits `QuestionResolved`
/// — the record the window and the conversation both keep — and returns
/// the answer to the handler. A question whose entry is dropped without
/// an answer resolves as declined: the persona should stop and wait,
/// which is what a decline says.
fn ask(
    state: &ChatState,
    ask: Ask,
    append: impl Fn(&TranscriptItem),
    emit: impl Fn(&TranscriptItem),
) -> Answer {
    let (id, receiver) = state.open_question();
    emit(&TranscriptItem::Question {
        id: id.clone(),
        ask: ask.clone(),
    });
    let answer = receiver.recv().unwrap_or(Answer::Declined);
    let resolved = TranscriptItem::QuestionResolved {
        id,
        ask,
        answer: answer.clone(),
    };
    append(&resolved);
    emit(&resolved);
    answer
}

/// Delivers `answer` to the handler waiting on question `id`. An id with
/// nothing pending — a stale card, a double click — is refused in words.
fn answer(state: &ChatState, id: &str, answer: Answer) -> Result<(), String> {
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

/// Appends the user's message and hands the new item to `emit`.
fn post(conversation: &mut Conversation, text: String, emit: impl FnOnce(&TranscriptItem)) {
    let item = TranscriptItem::Message {
        role: Role::User,
        text,
    };
    conversation.push(item.clone());
    emit(&item);
}

/// One turn against any replier, emitting what happens in the order it
/// happens: deltas while the model speaks, tool calls and their results
/// as the replier reports them — appended and emitted right then, like
/// messages — then the completed message, appended before it is emitted,
/// or `TurnFailed` with the reason. Generic over the replier and both
/// effects, which is what makes it testable without a wire or a window.
fn turn(
    reply: impl FnOnce(
        &mut dyn FnMut(&str),
        &mut dyn FnMut(TranscriptItem),
    ) -> Result<String, ChatError>,
    append: impl Fn(&TranscriptItem),
    emit: impl Fn(&TranscriptItem),
) {
    let mut on_delta = |delta: &str| {
        emit(&TranscriptItem::AssistantDelta {
            text: delta.to_owned(),
        });
    };
    let mut record = |item: TranscriptItem| {
        append(&item);
        emit(&item);
    };
    match reply(&mut on_delta, &mut record) {
        Ok(text) => {
            let message = TranscriptItem::Message {
                role: Role::Assistant,
                text,
            };
            append(&message);
            emit(&message);
        }
        Err(error) => emit(&TranscriptItem::TurnFailed {
            reason: error.to_string(),
        }),
    }
}

/// Posts the user's message, then holds the turn: the reply streams back
/// as transcript events while this command has long since returned.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, ChatState>,
    text: String,
) -> Result<(), String> {
    if !state.begin_turn() {
        return Err("a turn is already in flight".to_owned());
    }

    // Append and snapshot under one brief lock; the turn itself runs
    // without it.
    let snapshot: Vec<TranscriptItem> = {
        let mut conversation = state
            .conversation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        post(&mut conversation, text, |item| {
            let _ = app.emit(TRANSCRIPT_EVENT, item);
        });
        conversation.items().cloned().collect()
    };

    let key = match OsKeyring.resolve(API_KEY_NAME) {
        Resolved::Found(key) => key,
        Resolved::Absent => {
            let _ = app.emit(
                TRANSCRIPT_EVENT,
                &TranscriptItem::TurnFailed {
                    reason: format!("No Anthropic key is set. {SET_KEY_HINT}"),
                },
            );
            state.end_turn();
            return Ok(());
        }
        Resolved::Unreachable(reason) => {
            let _ = app.emit(
                TRANSCRIPT_EVENT,
                &TranscriptItem::TurnFailed {
                    reason: format!(
                        "The system keychain couldn't be reached ({reason}). {SET_KEY_HINT}"
                    ),
                },
            );
            state.end_turn();
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

    // The turn gets its own thread: an inline turn would hold this async
    // context — and the window's patience — for its whole duration.
    std::thread::spawn(move || {
        // The same key rides into a build agent's environment; the CLI's
        // own logged-in auth would also do, but the one the user set here
        // is the one they mean.
        let agent_key = Some(key.clone());
        let client = Client::anthropic(key);
        // Assembled fresh each turn, so a token pasted mid-session
        // reaches the very next turn.
        let mut registry = Registry::standard();
        registry.extend(epik::github::tools::all(GitHub::new(github_token.clone())));
        registry.extend(epik::git::all());
        registry.extend(crate::build::tools(app.clone(), agent_key.clone()));
        // The one question rail: whoever asks — the persona through
        // choose_repository, or the feature build machinery raising its
        // check card — blocks the turn's thread until the window answers.
        let asker = |app: AppHandle| {
            move |question| {
                let state = app.state::<ChatState>();
                ask(
                    &state,
                    question,
                    |item| state.append(item),
                    |item| {
                        let _ = app.emit(TRANSCRIPT_EVENT, item);
                    },
                )
            }
        };
        registry.extend(crate::build::feature_tools(
            &app,
            agent_key,
            github_token,
            asker(app.clone()),
        ));
        registry.register(choose_repository(asker(app.clone())));
        let stop = AtomicBool::new(false);
        let state = app.state::<ChatState>();
        turn(
            |on_delta, record| {
                tools::run(
                    &client,
                    &SYSTEM_PROMPT,
                    &snapshot,
                    &registry,
                    on_delta,
                    |call, result| {
                        record(TranscriptItem::tool_call(call));
                        record(TranscriptItem::tool_result(&call.name, result));
                    },
                    &stop,
                )
            },
            |item| state.append(item),
            |item| {
                let _ = app.emit(TRANSCRIPT_EVENT, item);
            },
        );
        state.end_turn();
    });
    Ok(())
}

/// The window's answer to a pending question. Refused in words when no
/// question with that id is waiting.
#[tauri::command]
pub async fn answer_question(
    state: State<'_, ChatState>,
    id: String,
    answer: Answer,
) -> Result<(), String> {
    self::answer(&state, &id, answer)
}

/// The whole transcript so far, for a window that has just opened its eyes.
#[tauri::command]
pub async fn get_transcript(state: State<'_, ChatState>) -> Result<Vec<TranscriptItem>, String> {
    Ok(state
        .conversation
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .items()
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn a_posted_message_is_appended_and_emitted_as_the_same_item() {
        let mut conversation = Conversation::default();
        let emitted = RefCell::new(Vec::new());

        post(&mut conversation, "hello".to_owned(), |item| {
            emitted.borrow_mut().push(item.clone());
        });

        let expected = TranscriptItem::Message {
            role: Role::User,
            text: "hello".to_owned(),
        };
        let kept: Vec<_> = conversation.items().cloned().collect();
        assert_eq!(kept, std::slice::from_ref(&expected));
        assert_eq!(*emitted.borrow(), [expected]);
    }

    #[test]
    fn a_turn_streams_deltas_then_appends_and_emits_the_reply() {
        let emitted = RefCell::new(Vec::new());
        let appended = RefCell::new(Vec::new());

        turn(
            |on_delta, _| {
                on_delta("Hel");
                on_delta("lo");
                Ok("Hello".to_owned())
            },
            |message| appended.borrow_mut().push(message.clone()),
            |item| emitted.borrow_mut().push(item.clone()),
        );

        let reply = TranscriptItem::Message {
            role: Role::Assistant,
            text: "Hello".to_owned(),
        };
        assert_eq!(
            *emitted.borrow(),
            [
                TranscriptItem::AssistantDelta {
                    text: "Hel".to_owned()
                },
                TranscriptItem::AssistantDelta {
                    text: "lo".to_owned()
                },
                reply.clone(),
            ]
        );
        assert_eq!(*appended.borrow(), [reply]);
    }

    #[test]
    fn a_failed_turn_emits_the_reason_and_appends_nothing() {
        let emitted = RefCell::new(Vec::new());
        let appended = RefCell::new(Vec::<TranscriptItem>::new());

        turn(
            |_, _| Err(ChatError::Api("Your credit balance is too low.".to_owned())),
            |message| appended.borrow_mut().push(message.clone()),
            |item| emitted.borrow_mut().push(item.clone()),
        );

        assert_eq!(
            *emitted.borrow(),
            [TranscriptItem::TurnFailed {
                reason: "Your credit balance is too low.".to_owned()
            }]
        );
        assert!(appended.borrow().is_empty());
    }

    #[test]
    fn recorded_tool_items_are_appended_and_emitted_as_they_happen() {
        let emitted = RefCell::new(Vec::new());
        let appended = RefCell::new(Vec::new());
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
            |item| appended.borrow_mut().push(item.clone()),
            |item| emitted.borrow_mut().push(item.clone()),
        );

        let reply = TranscriptItem::Message {
            role: Role::Assistant,
            text: "It's noon.".to_owned(),
        };
        let expected = [call, result, reply];
        assert_eq!(*emitted.borrow(), expected);
        assert_eq!(
            *appended.borrow(),
            expected,
            "tool items land in the conversation, exactly like messages"
        );
    }

    /// The whole backend wiring against the scripted model: the library
    /// loop drives a real client, and the turn observes it into items.
    #[test]
    fn a_scripted_tool_turn_flows_through_the_turn_as_items() {
        use epik::chat::scripted::{Fragment, Scripted, Turn as Script};

        let model = Scripted::spawn(vec![
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
            |on_delta, record| {
                tools::run(
                    &client,
                    "system",
                    &[TranscriptItem::Message {
                        role: Role::User,
                        text: "what time is it?".to_owned(),
                    }],
                    &registry,
                    on_delta,
                    |call, result| {
                        record(TranscriptItem::tool_call(call));
                        record(TranscriptItem::tool_result(&call.name, result));
                    },
                    &stop,
                )
            },
            |_| {},
            |item| emitted.borrow_mut().push(item.clone()),
        );

        let kinds: Vec<_> = emitted
            .borrow()
            .iter()
            .map(|item| match item {
                TranscriptItem::ToolCall { .. } => "call",
                TranscriptItem::ToolResult { ok: true, .. } => "ok",
                TranscriptItem::AssistantDelta { .. } => "delta",
                TranscriptItem::Message { .. } => "message",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["call", "ok", "delta", "delta", "message"]);
        assert_eq!(
            model.requests()[1]["messages"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["tool_call_id"],
            "call_1"
        );
    }

    #[test]
    fn the_turn_belongs_to_one_caller_until_returned() {
        let state = ChatState::default();
        assert!(state.begin_turn());
        assert!(!state.begin_turn(), "a second claim is refused");
        state.end_turn();
        assert!(state.begin_turn(), "a returned claim can be taken again");
    }

    fn where_to() -> Ask {
        Ask::Repository {
            prompt: "Where should this project live?".to_owned(),
        }
    }

    /// The whole ask round trip: the asker blocks on its own thread
    /// (the turn's, in the app), the Question goes out as an event, the
    /// answer arrives through the command's inner function, and the
    /// resolution is appended and emitted before the handler gets its
    /// answer back.
    #[test]
    fn an_ask_emits_the_question_and_resolves_when_answered() {
        use std::sync::Arc;

        let state = Arc::new(ChatState::default());
        let (events_in, events) = channel::<TranscriptItem>();
        let asker = std::thread::spawn({
            let state = Arc::clone(&state);
            move || {
                ask(
                    &state,
                    where_to(),
                    |item| state.append(item),
                    |item| {
                        let _ = events_in.send(item.clone());
                    },
                )
            }
        });

        let TranscriptItem::Question { id, ask: asked } = events.recv().unwrap() else {
            panic!("the first event is the question");
        };
        assert_eq!(asked, where_to());
        assert!(
            state.conversation.lock().unwrap().items().next().is_none(),
            "a pending question is not conversation"
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
            events.recv().unwrap(),
            resolved,
            "the resolution is emitted"
        );
        let kept: Vec<_> = state
            .conversation
            .lock()
            .unwrap()
            .items()
            .cloned()
            .collect();
        assert_eq!(kept, [resolved], "and appended");
        assert!(state.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn an_answer_to_no_pending_question_is_refused_in_words() {
        let state = ChatState::default();
        let error = answer(&state, "42", Answer::Declined).unwrap_err();
        assert!(error.contains("42"), "{error}");
        assert!(error.contains("no question"), "{error}");
    }

    #[test]
    fn a_question_answered_twice_is_refused_the_second_time() {
        let state = ChatState::default();
        let (id, receiver) = state.open_question();
        answer(&state, &id, Answer::Declined).unwrap();
        assert_eq!(receiver.recv().unwrap(), Answer::Declined);
        assert!(answer(&state, &id, Answer::Declined).is_err());
    }

    #[test]
    fn question_ids_are_distinct_and_monotonic() {
        let state = ChatState::default();
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
        use epik::chat::scripted::{Fragment, Scripted, Turn as Script};
        use std::sync::Arc;

        let model = Scripted::spawn(vec![
            Script::ToolCalls(vec![vec![Fragment::open(
                0,
                "call_1",
                "choose_repository",
                r#"{"prompt":"Where should this project live?"}"#,
            )]]),
            Script::text(&["Making it there."]),
        ]);
        let base_url = model.base_url();
        let state = Arc::new(ChatState::default());
        let (events_in, events) = channel::<TranscriptItem>();

        let turning = std::thread::spawn({
            let state = Arc::clone(&state);
            move || {
                let client = Client::new(base_url, "scripted".to_owned(), None);
                let mut registry = Registry::default();
                registry.register(choose_repository({
                    let state = Arc::clone(&state);
                    let events_in = events_in.clone();
                    move |question| {
                        ask(
                            &state,
                            question,
                            |item| state.append(item),
                            |item| {
                                let _ = events_in.send(item.clone());
                            },
                        )
                    }
                }));
                turn(
                    |on_delta, record| {
                        tools::run(
                            &client,
                            "system",
                            &[TranscriptItem::Message {
                                role: Role::User,
                                text: "write me hunt the wumpus".to_owned(),
                            }],
                            &registry,
                            on_delta,
                            |call, result| {
                                record(TranscriptItem::tool_call(call));
                                record(TranscriptItem::tool_result(&call.name, result));
                            },
                            &AtomicBool::new(false),
                        )
                    },
                    |item| state.append(item),
                    |item| {
                        let _ = events_in.send(item.clone());
                    },
                );
            }
        });

        let TranscriptItem::Question { id, ask: asked } = events.recv().unwrap() else {
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

        let kinds: Vec<_> = std::iter::once(TranscriptItem::Question {
            id: id.clone(),
            ask: asked,
        })
        .chain(events.try_iter())
        .map(|item| match item {
            TranscriptItem::Question { .. } => "question",
            TranscriptItem::QuestionResolved { .. } => "resolved",
            TranscriptItem::ToolCall { .. } => "call",
            TranscriptItem::ToolResult { ok: true, .. } => "ok",
            TranscriptItem::AssistantDelta { .. } => "delta",
            TranscriptItem::Message { .. } => "message",
            _ => "other",
        })
        .collect();
        assert_eq!(
            kinds,
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
