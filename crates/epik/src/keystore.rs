//! Where secrets live: not here.
//!
//! Secrets belong to the OS keyring — Keychain, Credential Manager, secret
//! service — under service [`SERVICE`], as a set of (name, secret) pairs.
//! Names are the callers' to choose; this module attaches no meaning to any
//! of them. Values exist to be *used*, at runtime, and for nothing else:
//! never printed, never logged, never written to a file. [`Secret`] is the
//! type that keeps that promise, and keeps every exception to it greppable.
//!
//! [`KeyStore`] is the seam that keeps each placement an independent
//! decision — and that keeps a test suite out of anybody's keychain.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::Result;

/// The keyring service every Epik secret is filed under.
pub const SERVICE: &str = "Epik";

/// A secret in hand: usable anywhere, visible nowhere.
///
/// The bytes come out through [`reveal`](Self::reveal) and no other door, so
/// every use of the raw value is greppable. `Debug` prints a redaction and
/// there is no `Display` at all: a secret cannot wander into an error
/// message, a panic, or a log line just by being formatted along the way.
///
/// Serialization is the one other door: the bare bytes, which is what
/// lets a secret cross a process boundary — the IPC barrier, an agent
/// runner's stdin — as itself instead of decaying into a `String` on
/// each side. That crossing is the accepted exposure — every place it
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

/// Somewhere a secret can be kept and found again.
pub trait KeyStore {
    /// The secret filed against `name`, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns an error when the store itself could not be consulted — which
    /// is a different thing from there being no secret.
    fn get(&self, name: &str) -> Result<Option<Secret>>;

    /// Files `secret` against `name`, replacing whatever was there.
    ///
    /// # Errors
    ///
    /// Returns an error when the store would not take it.
    fn set(&mut self, name: &str, secret: Secret) -> Result<()>;

    /// Where the secret for `name` stands, with a store that will not answer
    /// reported rather than raised.
    ///
    /// [`get`](Self::get) makes an unreachable store an error, which is the
    /// right shape for a caller about to need a secret. This is the right
    /// shape for one that only needs to know where it stands — chiefly an
    /// app starting up, which must not fail for want of a keyring. A machine
    /// with no secret service still has plenty to offer; whoever really
    /// needs the secret will say so in their own words soon enough.
    fn resolve(&self, name: &str) -> Resolved {
        match self.get(name) {
            Ok(Some(secret)) => Resolved::Found(secret),
            Ok(None) => Resolved::Absent,
            Err(error) => Resolved::Unreachable(format!("{error:#}")),
        }
    }
}

/// Where resolution got to.
///
/// Three states rather than an `Option`, because "there is no secret" and
/// "there is no way to find out" are different situations and a caller
/// usually wants to treat them differently.
///
/// With the `serde` feature this is also the wire shape of a resolution:
/// both sides of the IPC barrier speak this type, defined once, here.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    #[must_use]
    pub fn key(self) -> Option<Secret> {
        match self {
            Self::Found(secret) => Some(secret),
            Self::Absent | Self::Unreachable(_) => None,
        }
    }
}

/// A store that lives and dies with the process. Dependents' tests hand this
/// to code that wants a [`KeyStore`] — the entire reason the trait exists —
/// which is why it is a public export rather than a `cfg(test)` double like
/// `Unplugged`.
#[derive(Debug, Default)]
pub struct InMemory(BTreeMap<String, Secret>);

impl KeyStore for InMemory {
    fn get(&self, name: &str) -> Result<Option<Secret>> {
        Ok(self.0.get(name).cloned())
    }

    fn set(&mut self, name: &str, secret: Secret) -> Result<()> {
        self.0.insert(name.to_owned(), secret);
        Ok(())
    }
}

/// A store that has broken down rather than one that is merely empty: the
/// machine with no secret service — a container, a headless Linux box, a CI
/// runner. The unit tests unplug it to show the library coping.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Unplugged;

#[cfg(test)]
impl KeyStore for Unplugged {
    fn get(&self, _: &str) -> Result<Option<Secret>> {
        Err(anyhow::anyhow!("no default store has been set"))
    }

    fn set(&mut self, _: &str, _: Secret) -> Result<()> {
        Err(anyhow::anyhow!("no default store has been set"))
    }
}

/// The operating system's own secret holder.
#[cfg(feature = "native")]
#[derive(Debug, Default)]
pub struct OsKeyring;

#[cfg(feature = "native")]
impl OsKeyring {
    fn entry(name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, name)
            .map_err(|error| anyhow::anyhow!("opening the {SERVICE}/{name} keyring entry: {error}"))
    }
}

#[cfg(feature = "native")]
impl KeyStore for OsKeyring {
    fn get(&self, name: &str) -> Result<Option<Secret>> {
        match Self::entry(name)?.get_password() {
            Ok(secret) => Ok(Some(secret.into())),
            // No entry is the ordinary state of a fresh install, not a fault.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(anyhow::anyhow!(
                "reading the {SERVICE}/{name} secret from the keyring: {error}"
            )),
        }
    }

    fn set(&mut self, name: &str, secret: Secret) -> Result<()> {
        Self::entry(name)?
            .set_password(secret.reveal())
            .map_err(|error| {
                anyhow::anyhow!("storing the {SERVICE}/{name} secret in the keyring: {error}")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_name_has_no_secret_rather_than_an_error() {
        let store = InMemory::default();
        assert_eq!(store.get("anthropic").unwrap(), None);
    }

    #[test]
    fn a_stored_secret_comes_back() {
        let mut store = InMemory::default();
        store.set("anthropic", "sk-stored".into()).unwrap();
        assert_eq!(store.get("anthropic").unwrap(), Some("sk-stored".into()));
    }

    #[test]
    fn secrets_are_filed_per_name() {
        let mut store = InMemory::default();
        store.set("anthropic", "sk-anthropic".into()).unwrap();
        store.set("groq", "gsk-groq".into()).unwrap();
        assert_eq!(store.get("anthropic").unwrap(), Some("sk-anthropic".into()));
        assert_eq!(store.get("groq").unwrap(), Some("gsk-groq".into()));
    }

    #[test]
    fn resolution_reports_the_three_states_it_can_be_in() {
        let mut store = InMemory::default();
        store.set("kept", "sk-kept".into()).unwrap();

        assert_eq!(store.resolve("kept"), Resolved::Found("sk-kept".into()));
        assert_eq!(store.resolve("never-stored"), Resolved::Absent);
    }

    #[test]
    fn a_store_that_will_not_answer_is_a_state_rather_than_a_failure() {
        let store = Unplugged;

        let Resolved::Unreachable(reason) = store.resolve("anthropic") else {
            panic!("a broken store should resolve to Unreachable");
        };
        assert!(reason.contains("no default store"), "{reason}");

        assert!(
            store.get("anthropic").is_err(),
            "the strict reading is still available to a caller that wants it"
        );
    }

    #[test]
    fn resolution_yields_the_secret_a_caller_takes() {
        assert_eq!(Resolved::Found("sk-x".into()).key(), Some("sk-x".into()));
        assert_eq!(Resolved::Absent.key(), None);
        assert_eq!(Resolved::Unreachable("broken".to_owned()).key(), None);
    }

    #[test]
    fn debug_formatting_a_secret_yields_a_redaction_rather_than_the_bytes() {
        let secret = Secret::from("sk-super-secret");
        let debugged = format!("{secret:?}");

        assert!(!debugged.contains("sk-super-secret"), "{debugged}");
        assert_eq!(debugged, "[redacted]");
    }

    #[test]
    fn debug_formatting_a_store_never_yields_the_secrets_it_holds() {
        let mut store = InMemory::default();
        store.set("anthropic", "sk-super-secret".into()).unwrap();
        let debugged = format!("{store:?}");

        assert!(!debugged.contains("sk-super-secret"), "{debugged}");
        assert!(
            debugged.contains("anthropic"),
            "the name is not the secret: {debugged}"
        );
    }

    /// Serialization is the one deliberate door out: a resolution crosses
    /// the wire with its bytes intact and comes back the same resolution —
    /// while Debug keeps redacting on both sides of the trip.
    #[cfg(feature = "serde")]
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
