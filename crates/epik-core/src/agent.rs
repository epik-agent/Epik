//! Agents: the Processes that write code, and the typed reading of
//! what each one says. [`claude`] is the first.
//!
//! An [`Agent`] is data that becomes a [`Process`]: [`start`] is where
//! it turns into a command line and an environment. What the process
//! says on stdout is the engine's own protocol; [`interpret`] reads one
//! line of it into the [`Update`]s Epik understands, the same small
//! vocabulary whichever engine is speaking.
//!
//! [`start`]: Agent::start
//! [`interpret`]: Agent::interpret

use serde::{Deserialize, Serialize};

#[cfg(feature = "native")]
use crate::process::Process;

pub mod claude;

/// One thing Epik understands from an agent, in the order said. The
/// typed view is deliberately small: enough to narrate progress and
/// settle the outcome, no more.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum Update {
    /// The session opened: its id, and the model actually speaking.
    Session { id: String, model: String },
    /// The engine said something — progress narration.
    Text { text: String },
    /// The engine reached for a tool: enough to say "it's editing
    /// files" or "it's running tests", no more.
    ToolUse { name: String },
    /// The terminal line: how the run ended, in the engine's own words,
    /// with the bill when one was presented.
    Result {
        ok: bool,
        text: String,
        cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
}

/// A coding engine Epik can run: started as a [`Process`], read as
/// [`Update`]s.
#[cfg(feature = "native")]
pub trait Agent {
    /// Starts the engine in its working directory, with what it needs
    /// in its arguments and environment. Nothing flows in after that:
    /// a Process has no stdin.
    ///
    /// # Errors
    ///
    /// The spawn itself failing — the binary missing, chiefly.
    fn start(self) -> anyhow::Result<Process>;

    /// The updates one line of the engine's stdout carries — usually
    /// one, sometimes several, and none for a line Epik does not
    /// model. Never an error: whatever a newer engine says that this
    /// reading does not know, it says to nobody.
    fn interpret(line: &str) -> Vec<Update>;
}
