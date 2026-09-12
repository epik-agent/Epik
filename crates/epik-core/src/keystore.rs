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
}
