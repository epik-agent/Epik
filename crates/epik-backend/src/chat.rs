//! The chat round-trip. Commands carry intent in; events carry
//! observation out.
//!
//! [`send_message`] appends to the canonical [`Conversation`] and emits
//! the appended item on the transcript channel — the window shows nothing
//! it didn't receive back as an event. Nothing answers yet, so the
//! transcript gains user messages only, but this is the same plumbing a
//! model will stream into later.

use std::sync::{Mutex, PoisonError};

use epik::chat::{Conversation, Role, TRANSCRIPT_EVENT, TranscriptItem};
use tauri::{AppHandle, Emitter, State};

/// The one mutation, against any conversation and any emitter — which is
/// what makes it testable without a window.
fn post(conversation: &mut Conversation, text: String, emit: impl FnOnce(&TranscriptItem)) {
    let item = TranscriptItem::Message {
        role: Role::User,
        text,
    };
    conversation.push(item.clone());
    emit(&item);
}

/// Posts the user's message to the conversation. The window sees it when
/// the transcript event comes back around.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    conversation: State<'_, Mutex<Conversation>>,
    text: String,
) -> Result<(), String> {
    let mut conversation = conversation.lock().unwrap_or_else(PoisonError::into_inner);
    post(&mut conversation, text, |item| {
        let _ = app.emit(TRANSCRIPT_EVENT, item);
    });
    Ok(())
}

/// The whole transcript so far, for a window that has just opened its eyes.
#[tauri::command]
pub async fn get_transcript(
    conversation: State<'_, Mutex<Conversation>>,
) -> Result<Vec<TranscriptItem>, String> {
    Ok(conversation
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
    fn posts_accumulate_in_order() {
        let mut conversation = Conversation::default();
        post(&mut conversation, "first".to_owned(), |_| {});
        post(&mut conversation, "second".to_owned(), |_| {});

        let texts: Vec<_> = conversation
            .items()
            .map(|TranscriptItem::Message { text, .. }| text.as_str())
            .collect();
        assert_eq!(texts, ["first", "second"]);
    }
}
