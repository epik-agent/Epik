//! Where keys live: not here.
//!
//! Secrets are pushed to their official holders and Epik keeps references,
//! never values. A chat key belongs in the OS keyring — Keychain, Credential
//! Manager, secret service — under service `Epik`, with the provider's
//! configured name as the account. The GitHub token is the keyring's second
//! entry kind, on the same rails under the fixed account
//! [`GITHUB_ACCOUNT`]: a fine-grained PAT pasted once, never a line in a
//! config file. Anthropic auth stays inside the `claude` CLI's login, and CI
//! keys, when they come, in GitHub Actions secrets.
//!
//! [`KeyStore`] is the seam that keeps each of those placements an
//! independent decision — and that keeps a test suite out of anybody's
//! keychain.

use std::collections::BTreeMap;
use std::env;
use std::sync::{Arc, PoisonError, RwLock};

use anyhow::Result;

/// The keyring service every Epik secret is filed under.
pub const SERVICE: &str = "Epik";

/// An environment variable that outranks the keyring, for a shell session
/// that wants to use a different chat key without storing it anywhere.
///
/// It answers for every chat provider, and never for the GitHub token.
pub const OVERRIDE_ENV: &str = "EPIK_API_KEY";

/// The keyring account the GitHub token is filed under.
///
/// A fixed name rather than a configured one, because Epik talks to one
/// GitHub.
///
/// The account namespace is shared between entry kinds, so this name is
/// reserved across all of them: a chat provider configured as `github`
/// would share this entry — its `set` and [`set_github_token`] overwrite
/// each other, and its `resolve` hands back the token.
///
/// [`set_github_token`]: Keys::set_github_token
pub const GITHUB_ACCOUNT: &str = "github";

/// The environment variable that outranks the keyring for the GitHub token,
/// and for nothing else.
///
/// The two override envs are strangers by design: [`OVERRIDE_ENV`] never
/// speaks for GitHub, and this one never speaks for a chat provider.
pub const GITHUB_OVERRIDE_ENV: &str = "EPIK_GITHUB_TOKEN";

/// A key in hand, behind a shared handle: every clone reads the same cell,
/// so a key pasted mid-session reaches a client that was built before it
/// arrived.
///
/// This is the counterpart of [`Keys`], not a rival: the store is where a
/// key is *kept*, and this is a key *in use*. The GitHub client holds one
/// end and the session the other, which is what lets
/// `Session::set_github_token` re-credential verbs already registered
/// without rebuilding anything.
#[derive(Clone, Debug, Default)]
pub struct Credential(Arc<RwLock<Option<String>>>);

impl Credential {
    /// A credential holding `token` — or holding nothing, ready to be told
    /// one later.
    #[must_use]
    pub fn new(token: Option<String>) -> Self {
        Self(Arc::new(RwLock::new(token)))
    }

    /// Replaces the token for every holder of this handle at once.
    pub fn use_token(&self, token: Option<String>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = token;
    }

    /// The token as it stands. A poisoned lock is read through rather than
    /// raised: the cell holds a plain value, so whatever is in it is intact.
    #[must_use]
    pub fn token(&self) -> Option<String> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

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

/// Where resolution got to.
///
/// Three states rather than an `Option`, because "there is no key" and "there
/// is no way to find out" are different situations and a caller usually wants
/// to treat them differently.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolved {
    /// A key, from the environment or the store.
    Found(String),
    /// None, and nothing wrong. The ordinary state of a fresh install, and the
    /// permanent state of a local server that wants none.
    Absent,
    /// The store would not answer, so whether a key exists is unknown. The
    /// reason, for whoever has somewhere to say it.
    Unreachable(String),
}

impl Resolved {
    /// The key, if there is one. What a model's constructor takes.
    #[must_use]
    pub fn key(self) -> Option<String> {
        match self {
            Self::Found(key) => Some(key),
            Self::Absent | Self::Unreachable(_) => None,
        }
    }
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

/// A store that has broken down rather than one that is merely empty: the
/// machine with no secret service — a container, a headless Linux box, a CI
/// runner. The unit tests here and in `session` unplug it to show the
/// library coping, which is why it lives beside [`InMemory`] instead of
/// twice over.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Unplugged;

#[cfg(test)]
impl KeyStore for Unplugged {
    fn get(&self, _: &str) -> Result<Option<String>> {
        Err(anyhow::anyhow!("no default store has been set"))
    }

    fn set(&mut self, _: &str, _: &str) -> Result<()> {
        Err(anyhow::anyhow!("no default store has been set"))
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
/// The overrides are read once, when this is built, because a process-wide
/// override is what an environment variable means. They are read-only:
/// [`set`](Self::set) always writes to the store, and an override in force
/// keeps winning until the process ends.
#[derive(Debug)]
pub struct Keys<S> {
    store: S,
    override_key: Option<String>,
    github_override: Option<String>,
}

impl<S: KeyStore> Keys<S> {
    /// Wraps `store`, honouring `EPIK_API_KEY` and `EPIK_GITHUB_TOKEN` if
    /// they are set to anything.
    #[must_use]
    pub fn new(store: S) -> Self {
        let set = |name| env::var(name).ok().filter(|key: &String| !key.is_empty());
        Self::with_overrides(store, set(OVERRIDE_ENV), set(GITHUB_OVERRIDE_ENV))
    }

    /// Wraps `store` with a stated chat-key override and no GitHub one.
    #[must_use]
    pub const fn with_override(store: S, override_key: Option<String>) -> Self {
        Self::with_overrides(store, override_key, None)
    }

    /// Wraps `store` with both overrides stated — which is how the resolution
    /// order gets tested without a process mutating its own environment.
    #[must_use]
    pub const fn with_overrides(
        store: S,
        override_key: Option<String>,
        github_override: Option<String>,
    ) -> Self {
        Self {
            store,
            override_key,
            github_override,
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

    /// Where the key for `provider` stands, with a store that will not answer
    /// reported rather than raised.
    ///
    /// [`get`](Self::get) makes an unreachable store an error, which is the
    /// right shape for a caller about to need a key. This is the right shape
    /// for one that only needs to know where it stands — chiefly opening a
    /// session, which must not fail for want of a keyring. A client that cannot
    /// chat to a local model because a headless Linux box has no secret service
    /// has mistaken its own plumbing for the user's problem; the provider that
    /// does want a key will say so in its own words soon enough.
    ///
    /// The environment override is honoured first, so it works on a machine
    /// with no store at all.
    pub fn resolve(&self, provider: &str) -> Resolved {
        self.stand(self.override_key.as_deref(), provider)
    }

    /// Where the GitHub token stands: `EPIK_GITHUB_TOKEN`, then the keyring
    /// under [`GITHUB_ACCOUNT`], then absent.
    ///
    /// The same three states as [`resolve`](Self::resolve), for the same
    /// reason: a client without a token can still browse public repos, and a
    /// keyring that will not answer is something to render, never a reason to
    /// refuse. The chat-key override does not answer here — a shell that has
    /// exported `EPIK_API_KEY` has said nothing about GitHub.
    pub fn github_token(&self) -> Resolved {
        self.stand(self.github_override.as_deref(), GITHUB_ACCOUNT)
    }

    fn stand(&self, override_key: Option<&str>, account: &str) -> Resolved {
        if let Some(key) = override_key {
            return Resolved::Found(key.to_owned());
        }
        match self.store.get(account) {
            Ok(Some(key)) => Resolved::Found(key),
            Ok(None) => Resolved::Absent,
            Err(error) => Resolved::Unreachable(format!("{error:#}")),
        }
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

    /// Stores the GitHub token, filed under [`GITHUB_ACCOUNT`]. This is what
    /// the paste-your-PAT card calls; the config file never sees it.
    ///
    /// # Errors
    ///
    /// Returns an error when the store would not take it.
    pub fn set_github_token(&mut self, token: &str) -> Result<()> {
        self.store.set(GITHUB_ACCOUNT, token)
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
    fn resolution_reports_the_three_states_it_can_be_in() {
        let mut store = InMemory::default();
        store.set("kept", "sk-kept").unwrap();
        let keys = Keys::with_override(store, None);

        assert_eq!(keys.resolve("kept"), Resolved::Found("sk-kept".to_owned()));
        assert_eq!(keys.resolve("never-stored"), Resolved::Absent);
    }

    #[test]
    fn a_store_that_will_not_answer_is_a_state_rather_than_a_failure() {
        let keys = Keys::with_override(Unplugged, None);

        let Resolved::Unreachable(reason) = keys.resolve("anthropic") else {
            panic!("a broken store should resolve to Unreachable");
        };
        assert!(reason.contains("no default store"), "{reason}");

        assert!(
            keys.get("anthropic").is_err(),
            "the strict reading is still available to a caller that wants it"
        );
    }

    #[test]
    fn the_override_answers_even_with_no_store_to_speak_of() {
        let keys = Keys::with_override(Unplugged, Some("sk-override".to_owned()));

        assert_eq!(
            keys.resolve("anthropic"),
            Resolved::Found("sk-override".to_owned()),
            "a machine with no keyring can still be told a key"
        );
    }

    #[test]
    fn resolution_yields_the_key_a_model_constructor_takes() {
        assert_eq!(
            Resolved::Found("sk-x".to_owned()).key().as_deref(),
            Some("sk-x")
        );
        assert_eq!(Resolved::Absent.key(), None);
        assert_eq!(Resolved::Unreachable("broken".to_owned()).key(), None);
    }

    #[test]
    fn a_stored_github_token_round_trips() {
        let mut keys = Keys::with_override(InMemory::default(), None);

        assert_eq!(keys.github_token(), Resolved::Absent);
        keys.set_github_token("ghp-pasted").unwrap();
        assert_eq!(
            keys.github_token(),
            Resolved::Found("ghp-pasted".to_owned())
        );
    }

    #[test]
    fn the_github_environment_outranks_the_stored_token() {
        let mut keys = Keys::with_overrides(
            InMemory::default(),
            None,
            Some("ghp-from-the-environment".to_owned()),
        );
        keys.set_github_token("ghp-stored").unwrap();

        assert_eq!(
            keys.github_token(),
            Resolved::Found("ghp-from-the-environment".to_owned())
        );
    }

    #[test]
    fn the_chat_override_does_not_answer_for_github() {
        let keys = Keys::with_overrides(InMemory::default(), Some("sk-chat".to_owned()), None);
        assert_eq!(keys.github_token(), Resolved::Absent);
    }

    #[test]
    fn the_github_override_does_not_answer_for_a_chat_provider() {
        let keys = Keys::with_overrides(InMemory::default(), None, Some("ghp-token".to_owned()));
        assert_eq!(keys.resolve("anthropic"), Resolved::Absent);
    }

    #[test]
    fn the_two_entry_kinds_are_filed_apart() {
        let mut keys = Keys::with_override(InMemory::default(), None);
        keys.set("anthropic", "sk-chat").unwrap();
        keys.set_github_token("ghp-token").unwrap();

        assert_eq!(
            keys.resolve("anthropic"),
            Resolved::Found("sk-chat".to_owned())
        );
        assert_eq!(keys.github_token(), Resolved::Found("ghp-token".to_owned()));
    }

    #[test]
    fn a_github_token_the_store_cannot_reach_is_a_state_rather_than_a_failure() {
        let keys = Keys::with_override(Unplugged, None);

        let Resolved::Unreachable(reason) = keys.github_token() else {
            panic!("a broken store should resolve to Unreachable");
        };
        assert!(reason.contains("no default store"), "{reason}");
    }

    #[test]
    fn the_github_override_answers_even_with_no_store_to_speak_of() {
        let keys = Keys::with_overrides(Unplugged, None, Some("ghp-override".to_owned()));

        assert_eq!(
            keys.github_token(),
            Resolved::Found("ghp-override".to_owned()),
            "a machine with no keyring can still be told a token"
        );
    }

    #[test]
    fn every_clone_of_a_credential_reads_the_same_cell() {
        let held = Credential::new(None);
        let other_end = held.clone();

        held.use_token(Some("ghp-pasted".to_owned()));

        assert_eq!(
            other_end.token().as_deref(),
            Some("ghp-pasted"),
            "a token told to one end is in hand at the other"
        );
        other_end.use_token(None);
        assert_eq!(held.token(), None, "and taking it back works the same way");
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
