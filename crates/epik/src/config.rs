//! The configuration's shape: what the file can state, as the settings
//! window sees it.
//!
//! An omitted entry means the default, and an omitted section means all
//! of its entries are: every entry is an `Option`, absent reads as `None`,
//! and `None` serializes as absence, so a section holding nothing leaves
//! no empty `[table]` header behind.
//!
//! Wire shape as much as file shape: [`Config`] crosses the IPC barrier
//! to the settings window and back, so it is compiled everywhere `serde`
//! is. The file itself — its path, its format, the startup that converges
//! on it — belongs to the backend that keeps it.

use serde::{Deserialize, Serialize};

/// Everything the file can state. A section missing from the file is a
/// section of `None`s; a key the file states that is not named here is
/// a parse error, so a file in some other shape is reported rather than
/// quietly read as empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Model::is_default")]
    pub model: Model,
    #[serde(default, skip_serializing_if = "GitHub::is_default")]
    pub github: GitHub,
}

/// `[model]`: which models the chat window and the build Agents speak to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    /// The chat window's model; `None` is
    /// [`ANTHROPIC_MODEL`](crate::chat::ANTHROPIC_MODEL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// The build Agent's model; `None` lets the agent CLI pick its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// `[github]`: where a bare repository name is looked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHub {
    /// The owner a bare repository name settles against; `None` refuses
    /// bare names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

impl Model {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl GitHub {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
