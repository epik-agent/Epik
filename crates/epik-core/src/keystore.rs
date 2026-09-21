//! Where secrets live: not here.
//!
//! Values exist to be *used*, at runtime, and for nothing else: never
//! printed, never logged, never written to a file. [`Secret`] is the type
//! that keeps that promise, and keeps every exception to it greppable.

use std::fmt;

/// A secret in hand: usable anywhere, visible nowhere.
///
/// The bytes come out through [`reveal`](Self::reveal) and no other door, so
/// every use of the raw value is greppable. `Debug` prints a redaction and
/// there is no `Display` at all: a secret cannot wander into an error
/// message, a panic, or a log line just by being formatted along the way.
///
/// Serialization is the one other door: the bare bytes, which is what
/// lets a secret cross a process boundary — the IPC barrier — as itself
/// instead of decaying into a `String` on each side. That crossing is the accepted exposure — every place it
/// can happen types itself `Secret` and is findable by that name.
#[derive(Clone, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Secret(String);

impl Secret {
    /// The bytes themselves, for the moment of use — an HTTP header, a
    /// keyring write. Holding the `&str` any longer than that defeats the
    /// wrapper.
    #[must_use]
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Where resolution got to.
///
/// Three states rather than an `Option`, because "there is no secret" and
/// "there is no way to find out" are different situations and a caller
/// usually wants to treat them differently.
///
/// This is also the wire shape of a resolution: both sides of the IPC
/// barrier speak this type, defined once, here.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Resolved {
    /// A secret, from the store.
    Found(Secret),
    /// None, and nothing wrong. The ordinary state of a fresh install.
    Absent,
    /// The store would not answer, so whether a secret exists is unknown.
    /// The reason, for whoever has somewhere to say it.
    Unreachable(String),
}

impl Resolved {
    /// The secret, if there is one. What a caller about to use it takes.
    #[cfg(test)]
    fn key(self) -> Option<Secret> {
        match self {
            Self::Found(secret) => Some(secret),
            Self::Absent | Self::Unreachable(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_formatting_a_secret_yields_a_redaction_rather_than_the_bytes() {
        let secret = Secret::from("sk-super-secret");
        let debugged = format!("{secret:?}");

        assert!(!debugged.contains("sk-super-secret"), "{debugged}");
        assert_eq!(debugged, "[redacted]");
    }

    #[test]
    fn resolution_yields_the_secret_a_caller_takes() {
        assert_eq!(Resolved::Found("sk-x".into()).key(), Some("sk-x".into()));
        assert_eq!(Resolved::Absent.key(), None);
        assert_eq!(Resolved::Unreachable("broken".to_owned()).key(), None);
    }

    /// Serialization is the one deliberate door out: a resolution crosses
    /// the wire with its bytes intact and comes back the same resolution —
    /// while Debug keeps redacting on both sides of the trip.
    #[test]
    fn a_resolution_crosses_the_wire_and_comes_back_itself() {
        let sent = Resolved::Found("sk-super-secret".into());
        let wire = serde_json::to_string(&sent).unwrap();
        let received: Resolved = serde_json::from_str(&wire).unwrap();

        assert!(wire.contains("sk-super-secret"), "the wire carries bytes");
        assert_eq!(received, sent);
        assert!(!format!("{received:?}").contains("sk-super-secret"));
    }
}
