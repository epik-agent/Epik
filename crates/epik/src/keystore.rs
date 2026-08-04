//! Where chat keys live: not here.
//!
//! Secrets are pushed to their official holders and Epik keeps references,
//! never values. A chat key belongs in the OS keyring — Keychain, Credential
//! Manager, secret service — under service `Epik`, with the provider's
//! configured name as the account. Anthropic auth stays inside the `claude`
//! CLI's login, GitHub auth inside `gh`, and CI keys, when they come, in
//! GitHub Actions secrets.
//!
//! [`KeyStore`] is the seam that keeps each of those placements an
//! independent decision — and that keeps a test suite out of anybody's
//! keychain.

use std::collections::BTreeMap;
use std::env;

use anyhow::Result;

/// The keyring service every Epik secret is filed under.
pub const SERVICE: &str = "Epik";

/// An environment variable that outranks the keyring, for a shell session
/// that wants to use a different key without storing it anywhere.
pub const OVERRIDE_ENV: &str = "EPIK_API_KEY";

/// Somewhere a provider's key can be kept and found again.
pub trait KeyStore {
    /// The key filed against `provider`, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns an error when the store itself could not be consulted — which
    /// is a different thing from there being no key.
    fn get(&self, provider: &str) -> Result<Option<String>>;

    /// Files `key` against `provider`, replacing whatever was there.
    ///
    /// # Errors
    ///
    /// Returns an error when the store would not take it.
    fn set(&mut self, provider: &str, key: &str) -> Result<()>;
}

/// A store that lives and dies with the process. Tests use this one, which is
/// the entire reason the trait exists.
#[derive(Debug, Default)]
pub struct InMemory(BTreeMap<String, String>);

impl KeyStore for InMemory {
    fn get(&self, provider: &str) -> Result<Option<String>> {
        Ok(self.0.get(provider).cloned())
    }

    fn set(&mut self, provider: &str, key: &str) -> Result<()> {
        self.0.insert(provider.to_owned(), key.to_owned());
        Ok(())
    }
}

/// The operating system's own secret holder.
#[cfg(feature = "native")]
#[derive(Debug, Default)]
pub struct OsKeyring;

#[cfg(feature = "native")]
impl OsKeyring {
    fn entry(provider: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, provider).map_err(|error| {
            anyhow::anyhow!("opening the {SERVICE}/{provider} keyring entry: {error}")
        })
    }
}

#[cfg(feature = "native")]
impl KeyStore for OsKeyring {
    fn get(&self, provider: &str) -> Result<Option<String>> {
        match Self::entry(provider)?.get_password() {
            Ok(key) => Ok(Some(key)),
            // No entry is the ordinary state of a fresh install, not a fault.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(anyhow::anyhow!(
                "reading the {SERVICE}/{provider} key from the keyring: {error}"
            )),
        }
    }

    fn set(&mut self, provider: &str, key: &str) -> Result<()> {
        Self::entry(provider)?.set_password(key).map_err(|error| {
            anyhow::anyhow!("storing the {SERVICE}/{provider} key in the keyring: {error}")
        })
    }
}

/// Key resolution, in one place: the environment override, then the store,
/// then absent — and absent is a state the app renders rather than an error.
///
/// The override is read once, when this is built, because a process-wide
/// override is what an environment variable means. It is read-only:
/// [`set`](Self::set) always writes to the store, and an override in force
/// keeps winning until the process ends.
#[derive(Debug)]
pub struct Keys<S> {
    store: S,
    override_key: Option<String>,
}

impl<S: KeyStore> Keys<S> {
    /// Wraps `store`, honouring `EPIK_API_KEY` if it is set to anything.
    #[must_use]
    pub fn new(store: S) -> Self {
        Self::with_override(
            store,
            env::var(OVERRIDE_ENV).ok().filter(|key| !key.is_empty()),
        )
    }

    /// Wraps `store` with a stated override — which is how the resolution
    /// order gets tested without a process mutating its own environment.
    #[must_use]
    pub const fn with_override(store: S, override_key: Option<String>) -> Self {
        Self {
            store,
            override_key,
        }
    }

    /// The key to use for `provider`, or `None` when there is none to use.
    ///
    /// # Errors
    ///
    /// Returns an error when the store could not be consulted.
    pub fn get(&self, provider: &str) -> Result<Option<String>> {
        if let Some(key) = &self.override_key {
            return Ok(Some(key.clone()));
        }
        self.store.get(provider)
    }

    /// Stores a key for `provider`. This is what the paste-your-key card
    /// calls, and after it the chat proceeds without a restart.
    ///
    /// # Errors
    ///
    /// Returns an error when the store would not take it.
    pub fn set(&mut self, provider: &str, key: &str) -> Result<()> {
        self.store.set(provider, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_provider_has_no_key_rather_than_an_error() {
        let keys = Keys::with_override(InMemory::default(), None);
        assert_eq!(keys.get("anthropic").unwrap(), None);
    }

    #[test]
    fn a_stored_key_comes_back() {
        let mut keys = Keys::with_override(InMemory::default(), None);
        keys.set("anthropic", "sk-stored").unwrap();
        assert_eq!(keys.get("anthropic").unwrap().as_deref(), Some("sk-stored"));
    }

    #[test]
    fn keys_are_filed_per_provider() {
        let mut keys = Keys::with_override(InMemory::default(), None);
        keys.set("anthropic", "sk-anthropic").unwrap();
        keys.set("groq", "gsk-groq").unwrap();
        assert_eq!(
            keys.get("anthropic").unwrap().as_deref(),
            Some("sk-anthropic")
        );
        assert_eq!(keys.get("groq").unwrap().as_deref(), Some("gsk-groq"));
    }

    #[test]
    fn the_environment_outranks_the_store() {
        let mut store = InMemory::default();
        store.set("anthropic", "sk-stored").unwrap();
        let keys = Keys::with_override(store, Some("sk-from-the-environment".to_owned()));

        assert_eq!(
            keys.get("anthropic").unwrap().as_deref(),
            Some("sk-from-the-environment")
        );
    }

    #[test]
    fn the_environment_answers_for_every_provider() {
        let keys = Keys::with_override(InMemory::default(), Some("sk-override".to_owned()));
        assert_eq!(
            keys.get("never-configured").unwrap().as_deref(),
            Some("sk-override")
        );
    }

    #[test]
    fn storing_a_key_under_an_override_stores_it_anyway() {
        let mut keys = Keys::with_override(InMemory::default(), Some("sk-override".to_owned()));

        keys.set("anthropic", "sk-pasted").unwrap();

        assert_eq!(
            keys.get("anthropic").unwrap().as_deref(),
            Some("sk-override"),
            "the override keeps winning for this process"
        );
        assert_eq!(
            keys.store.get("anthropic").unwrap().as_deref(),
            Some("sk-pasted"),
            "but the pasted key was kept, so the next process finds it"
        );
    }
}
