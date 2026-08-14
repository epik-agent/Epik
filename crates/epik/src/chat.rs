//! The chat vocabulary: who said what, in the order it was said.
//!
//! [`TranscriptItem`] is the unit that crosses the IPC barrier and gets
//! folded into a window's view of the conversation. Its serde form is
//! tagged, and that tag is the forward-compatibility contract: new kinds
//! of item — streamed deltas, tool calls — become new variants on the same
//! event channel and new arms in the same fold, never a schema break. One
//! variant exists today, because one kind of thing can happen today.

/// A speaker, by its wire-protocol name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum Role {
    User,
    Assistant,
}

/// One entry in a conversation's transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "kind", rename_all = "lowercase")
)]
pub enum TranscriptItem {
    /// A complete utterance, said by `role`.
    Message { role: Role, text: String },
}

/// The event channel transcript items arrive on, backend to window.
pub const TRANSCRIPT_EVENT: &str = "transcript";

/// The canonical transcript: what was actually said, in order. One holder
/// per conversation — in the app, the backend.
#[derive(Debug, Default)]
pub struct Conversation(Vec<TranscriptItem>);

impl Conversation {
    /// Appends `item`. The transcript only ever grows.
    pub fn push(&mut self, item: TranscriptItem) {
        self.0.push(item);
    }

    /// The transcript, oldest first.
    pub fn items(&self) -> std::slice::Iter<'_, TranscriptItem> {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: Role, text: &str) -> TranscriptItem {
        TranscriptItem::Message {
            role,
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_conversation_keeps_items_in_the_order_they_were_said() {
        let mut conversation = Conversation::default();
        conversation.push(message(Role::User, "first"));
        conversation.push(message(Role::Assistant, "second"));
        conversation.push(message(Role::User, "third"));

        let texts: Vec<_> = conversation
            .items()
            .map(|TranscriptItem::Message { text, .. }| text.as_str())
            .collect();
        assert_eq!(texts, ["first", "second", "third"]);
    }

    #[test]
    fn a_fresh_conversation_has_nothing_to_say() {
        assert_eq!(Conversation::default().items().count(), 0);
    }

    /// The tag in the JSON is the forward-compatibility contract: a future
    /// item kind is a new tag value, and an old reader can see what it is
    /// not, rather than misread what it is.
    #[cfg(feature = "serde")]
    #[test]
    fn a_transcript_item_crosses_the_wire_tagged() {
        let item = message(Role::User, "hello");
        let wire = serde_json::to_string(&item).unwrap();

        assert!(wire.contains(r#""kind":"message""#), "{wire}");
        assert!(wire.contains(r#""role":"user""#), "{wire}");

        let received: TranscriptItem = serde_json::from_str(&wire).unwrap();
        assert_eq!(received, item);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn both_roles_speak_their_wire_protocol_names() {
        let assistant = serde_json::to_string(&Role::Assistant).unwrap();
        assert_eq!(assistant, r#""assistant""#);
        let user = serde_json::to_string(&Role::User).unwrap();
        assert_eq!(user, r#""user""#);
    }
}
