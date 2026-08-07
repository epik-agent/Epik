//! The names the two ends of an IPC boundary agree on.
//!
//! They live in the library for the same reason the event types do: a name
//! that two crates spell separately is a name that drifts. The host reads
//! these to register its commands, the frontend reads them to call them, and
//! neither is free to rename one alone.
//!
//! What is here is protocol, not transport. How a name is dispatched is the
//! host's business — Tauri's command bridge in the window, something else in
//! the daemon — but *which* intents exist is the library's.

/// The one event channel.
///
/// Every vocabulary travels it, wrapped in an
/// [`Envelope`](crate::event::Envelope) that says which conversation an event
/// belongs to. A second vocabulary is a second match arm at the far end, not a
/// second channel.
pub const CHANNEL: &str = "epik://events";

/// One command per intent, and one query.
pub mod command {
    /// Who is answering, and whether there is a key to reach them with. A
    /// query rather than an intent: asked on mount, and again once a key has
    /// been stored.
    pub const STATUS: &str = "status";

    /// One user turn. Answers whether the turn could be started; the reply
    /// itself arrives on the event channel.
    pub const SEND_MESSAGE: &str = "send_message";

    /// A key for the active provider, to be filed and put into use at once.
    pub const SET_API_KEY: &str = "set_api_key";

    /// A GitHub token, to be filed on its own rails and put into use at
    /// once. The paste-your-PAT card's one call.
    pub const SET_GITHUB_TOKEN: &str = "set_github_token";
}
