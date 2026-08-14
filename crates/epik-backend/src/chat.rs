//! The chat round-trip. Commands carry intent in; events carry
//! observation out.
//!
//! [`send_message`] appends the user's message to the canonical
//! [`Conversation`], emits it back on the transcript channel, and runs
//! the model turn on its own thread — deltas stream out as
//! `AssistantDelta` events, and the completed reply is appended and
//! emitted as a `Message`. A turn that fails emits `TurnFailed` instead;
//! the window shows nothing it didn't receive as an event. One turn in
//! flight at a time: a send during a turn is refused on the command
//! channel, because a misplaced intent is not a chat failure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use epik::chat::{
    ChatError, Client, Conversation, Role, SYSTEM_PROMPT, TRANSCRIPT_EVENT, TranscriptItem,
};
use epik::keystore::{KeyStore, OsKeyring, Resolved};
use tauri::{AppHandle, Emitter, Manager, State};

/// The keystore entry the turn's key comes from.
const API_KEY_NAME: &str = "ANTHROPIC_API_KEY";

/// Where to send someone whose turn failed for want of a key.
const SET_KEY_HINT: &str = "Set the Anthropic key in Settings (Cmd+,).";

/// The chat's whole state: the canonical transcript, and whether a turn
/// is in flight.
#[derive(Default)]
pub struct ChatState {
    conversation: Mutex<Conversation>,
    turning: AtomicBool,
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
/// happens: deltas while the model speaks, then the completed message —
/// appended to the conversation before it is emitted — or `TurnFailed`
/// with the reason. Generic over the replier and both effects, which is
/// what makes it testable without a wire or a window.
fn turn(
    reply: impl FnOnce(&mut dyn FnMut(&str)) -> Result<String, ChatError>,
    append: impl FnOnce(&TranscriptItem),
    emit: impl Fn(&TranscriptItem),
) {
    let mut on_delta = |delta: &str| {
        emit(&TranscriptItem::AssistantDelta {
            text: delta.to_owned(),
        });
    };
    match reply(&mut on_delta) {
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

    // The turn gets its own thread: an inline turn would hold this async
    // context — and the window's patience — for its whole duration.
    std::thread::spawn(move || {
        let client = Client::anthropic(key);
        let stop = AtomicBool::new(false);
        let state = app.state::<ChatState>();
        turn(
            |on_delta| client.reply(&SYSTEM_PROMPT, &snapshot, on_delta, &stop),
            |message| {
                state
                    .conversation
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(message.clone());
            },
            |item| {
                let _ = app.emit(TRANSCRIPT_EVENT, item);
            },
        );
        state.end_turn();
    });
    Ok(())
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
            |on_delta| {
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
            |_| Err(ChatError::Api("Your credit balance is too low.".to_owned())),
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
    fn the_turn_belongs_to_one_caller_until_returned() {
        let state = ChatState::default();
        assert!(state.begin_turn());
        assert!(!state.begin_turn(), "a second claim is refused");
        state.end_turn();
        assert!(state.begin_turn(), "a returned claim can be taken again");
    }
}
