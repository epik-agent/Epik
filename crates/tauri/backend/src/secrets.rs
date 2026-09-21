//! The narrow door between the keystore and the window.
//!
//! Secret bytes cross IPC in exactly two places — [`secret_reveal`]'s answer
//! and [`secret_save`]'s argument. That exposure is accepted, known debt to
//! be closed; keeping the hole this narrow is the module's whole discipline.
//! Nothing else may carry a secret: no events, no logs, no further types.
//! Both crossings speak the library's own types — [`Resolved`] out,
//! [`Secret`] in — so the wire shape is defined once, in `epik-core`, for
//! both sides of the barrier.
//!
//! Secrets belong to the OS keyring, as a set of (name, secret) pairs.
//! Names are the callers' to choose; this module attaches no meaning to
//! any of them. [`KeyStore`] is the seam that keeps each placement an
//! independent decision — and that keeps a test suite out of anybody's
//! keychain.
//!
//! The store itself is [`OsKeyring`]: the operating system's own secret
//! holder — Keychain, Credential Manager, secret service — with every
//! Epik secret filed under service [`SERVICE`]. The commands are wiring
//! and nothing more. Everything they do is a keystore call, so
//! everything they do is testable against `InMemory` without a keychain
//! in sight.

use anyhow::Result;
use epik_core::keystore::{Resolved, Secret};

/// Somewhere a secret can be kept and found again.
pub(crate) trait KeyStore {
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

/// A store that lives and dies with the process: what the tests hand to
/// code that wants a [`KeyStore`].
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct InMemory(std::collections::BTreeMap<String, Secret>);

#[cfg(test)]
impl KeyStore for InMemory {
    fn get(&self, name: &str) -> Result<Option<Secret>> {
        Ok(self.0.get(name).cloned())
    }

    fn set(&mut self, name: &str, secret: Secret) -> Result<()> {
        self.0.insert(name.to_owned(), secret);
        Ok(())
    }
}

/// The keyring service every Epik secret is filed under.
const SERVICE: &str = "Epik";

/// The operating system's own secret holder.
#[derive(Debug, Default)]
pub(crate) struct OsKeyring;

impl OsKeyring {
    fn entry(name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, name)
            .map_err(|error| anyhow::anyhow!("opening the {SERVICE}/{name} keyring entry: {error}"))
    }
}

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

/// [`secret_save`] against any store, which is what makes it testable.
fn save(store: &mut impl KeyStore, name: &str, secret: Secret) -> Result<(), String> {
    store
        .set(name, secret)
        .map_err(|error| format!("{error:#}"))
}

/// The secret filed under `name`, for the settings page to prefill.
///
/// Async so the keyring's answer — which on macOS can involve a permission
/// dialog — never stalls the window's event loop.
#[tauri::command]
pub async fn secret_reveal(name: String) -> Resolved {
    OsKeyring.resolve(&name)
}

/// Files `value` under `name` in the OS keyring. The error, when there is
/// one, names the store and the entry — never the value.
#[tauri::command]
pub async fn secret_save(name: String, value: Secret) -> Result<(), String> {
    save(&mut OsKeyring, &name, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::Broken;

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
        let Resolved::Unreachable(reason) = Broken.resolve("anthropic") else {
            panic!("a broken store should resolve to Unreachable");
        };
        assert!(reason.contains("locked"), "{reason}");

        assert!(
            Broken.get("anthropic").is_err(),
            "the strict reading is still available to a caller that wants it"
        );
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

    #[test]
    fn save_writes_through_to_the_store() {
        let mut store = InMemory::default();
        save(&mut store, "anthropic", "sk-pasted".into()).unwrap();
        assert_eq!(store.get("anthropic").unwrap(), Some("sk-pasted".into()));
    }

    #[test]
    fn a_failed_save_surfaces_its_reason() {
        let reason = save(&mut Broken, "anthropic", "sk-pasted".into()).unwrap_err();
        assert!(reason.contains("locked"), "{reason}");
    }
}
