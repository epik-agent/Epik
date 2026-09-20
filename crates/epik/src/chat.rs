//! The assistant, by its old name: everything here is
//! [`epik_core::assistant`]'s, re-exported for the code that still says
//! `chat`.

pub use epik_core::assistant::{
    ANTHROPIC_MODEL, Answer, Ask, Conversation, Role, TRANSCRIPT_EVENT, TranscriptItem,
};
#[cfg(feature = "native")]
pub use epik_core::assistant::{Client, run};
