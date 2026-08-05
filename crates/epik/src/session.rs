//! One client's chat session: the config read, the key resolved, the model
//! built, the transcript kept.
//!
//! This module exists because of the invariant: anything the app can do must
//! be achievable through calls into this crate alone. Without it, the wiring
//! between [`Config`], [`Keys`] and a [`ChatModel`] would live in the Tauri
//! host, and the daemon would have to reinvent it. With it, a host is three
//! commands over three library calls, and every future thin client opens a
//! chat the same way.
//!
//! [`Status`] is the part that crosses IPC: what the status bar names, and
//! whether there is a key to reach it with. Never the key itself.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::chat::{ChatModel, Conversation, Keyed, Message, Reply, StopToken};
use crate::config::{Config, Provider};
use crate::event::ChatEvent;
use crate::keystore::{KeyStore, Keys, Resolved};
use crate::logging::Log;

/// Whether there is a key to reach the provider with: the answer, never the
/// key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Key {
    /// One resolved. Nothing to ask for.
    Present,
    /// None, and nothing wrong. The ordinary state of a fresh install, and the
    /// permanent state of a local server that wants none.
    Absent,
    /// The keyring would not answer, so whether one exists is unknown.
    ///
    /// Not a reason to refuse a session: a local model needs no key, and a
    /// provider that wants one will refuse the turn in its own words. It is
    /// worth saying, though — chiefly because a key pasted onto a machine in
    /// this state will not be kept either.
    Unreachable { reason: String },
}

impl Key {
    /// Whether to ask for one.
    #[must_use]
    pub const fn wanted(&self) -> bool {
        matches!(self, Self::Absent | Self::Unreachable { .. })
    }

    /// Why the keyring could not be consulted, when it could not be.
    #[must_use]
    pub fn trouble(&self) -> Option<&str> {
        match self {
            Self::Unreachable { reason } => Some(reason),
            Self::Present | Self::Absent => None,
        }
    }
}

/// What a client needs to know about the session it is in.
///
/// A plain serde type, because it crosses the IPC boundary: the status bar
/// names `model`, and `key` is what decides whether the chat pane shows a
/// paste-your-key card. The key itself stays where the operating system put it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Status {
    /// The active provider's configured name — which is also the account its
    /// key is filed under.
    pub provider: String,
    /// The model the provider was asked for, verbatim from the config.
    pub model: String,
    /// Where the key for it stands.
    pub key: Key,
}

/// A live chat session.
///
/// It owns the conversation and the key store together, because the one
/// operation that touches both — a key arriving mid-session and taking effect
/// without a restart — has to be one call rather than a sequence a host could
/// get wrong.
#[derive(Debug)]
pub struct Session<M, S> {
    status: Status,
    conversation: Conversation<M>,
    keys: Keys<S>,
}

impl<M: ChatModel + Keyed, S: KeyStore> Session<M, S> {
    /// A session on the active provider, with `build` making the model that
    /// speaks to it.
    ///
    /// [`open`](Session::open) is this with the OpenAI-compatible client. A
    /// native Anthropic client would arrive the same way, and so does the
    /// scripted model in a test — which is the point of taking the constructor
    /// rather than naming a type.
    ///
    /// # Errors
    ///
    /// Returns an error when the config names an active provider it does not
    /// list. Neither a missing key nor an unreachable keyring is an error:
    /// [`Status::key`] reports where things stand, and the client decides what
    /// to do about it.
    pub fn with_model(
        config: &Config,
        keys: Keys<S>,
        build: impl FnOnce(&Provider, Option<String>) -> M,
    ) -> Result<Self> {
        let (name, provider) = config.provider()?;
        let resolved = keys.resolve(name);
        let status = Status {
            provider: name.to_owned(),
            model: provider.model.clone(),
            key: match &resolved {
                Resolved::Found(_) => Key::Present,
                Resolved::Absent => Key::Absent,
                Resolved::Unreachable(reason) => Key::Unreachable {
                    reason: reason.clone(),
                },
            },
        };
        let model = build(provider, resolved.key());
        Ok(Self {
            status,
            conversation: Conversation::new(config.system_prompt.clone(), model),
            keys,
        })
    }

    /// Who is answering, and whether there is a key to reach them with.
    #[must_use]
    pub const fn status(&self) -> &Status {
        &self.status
    }

    /// The canonical transcript. A client's own is a view folded from events;
    /// this is the truth.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        self.conversation.messages()
    }

    /// The model, for a caller that needs to look at it — chiefly a test.
    #[must_use]
    pub const fn model(&self) -> &M {
        self.conversation.model()
    }

    /// One turn: see [`Conversation::send`].
    ///
    /// # Errors
    ///
    /// Returns the model's error when the turn could not be completed. A host
    /// reports that through the event stream and swallows the error, which is
    /// what keeps streamed failures off the command channel.
    pub fn send(
        &mut self,
        text: impl Into<String>,
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply> {
        self.conversation.send(text, log, stop)
    }

    /// Files `key` against the active provider and puts it into use at once.
    ///
    /// This is what the paste-your-key card calls, and the reason the chat
    /// proceeds without a restart: the conversation keeps its transcript and
    /// only the model's credential changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the store would not take the key, or could not
    /// then be read back.
    pub fn set_key(&mut self, key: &str) -> Result<&Status> {
        self.keys.set(&self.status.provider, key)?;
        // Read back rather than used as pasted: an `EPIK_API_KEY` override
        // outranks the store and keeps winning for this process, so what the
        // session uses has to be whatever actually resolves. The store having
        // just taken the key, `get` is the strict reading to want here.
        let resolved = self.keys.get(&self.status.provider)?;
        self.status.key = if resolved.is_some() {
            Key::Present
        } else {
            Key::Absent
        };
        self.conversation.model_mut().use_key(resolved);
        Ok(&self.status)
    }
}

#[cfg(feature = "native")]
impl<S: KeyStore> Session<crate::chat::OpenAiCompatible, S> {
    /// A session on the active provider, over the OpenAI chat-completions
    /// protocol: the whole of what starting a chat takes.
    ///
    /// # Errors
    ///
    /// As [`with_model`](Session::with_model).
    pub fn open(config: &Config, keys: Keys<S>) -> Result<Self> {
        Self::with_model(config, keys, crate::chat::OpenAiCompatible::new)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::chat::Scripted;
    use crate::keystore::InMemory;
    use crate::logging::Silent;

    const PERSONA: &str = "You are Epik.";

    fn config() -> Config {
        Config {
            active: "local".to_owned(),
            system_prompt: PERSONA.to_owned(),
            providers: BTreeMap::from([(
                "local".to_owned(),
                Provider {
                    base_url: "http://localhost:11434/v1".to_owned(),
                    model: "smollm2:135m".to_owned(),
                },
            )]),
        }
    }

    /// A session on the scripted model, with an empty store and no override.
    fn session(model: Scripted) -> Session<Scripted, InMemory> {
        Session::with_model(
            &config(),
            Keys::with_override(InMemory::default(), None),
            |_, key| {
                let mut model = model;
                model.use_key(key);
                model
            },
        )
        .expect("the config names a provider it lists")
    }

    #[test]
    fn the_status_names_the_model_the_status_bar_will_show() {
        let session = session(Scripted::default());
        assert_eq!(session.status().provider, "local");
        assert_eq!(session.status().model, "smollm2:135m");
    }

    #[test]
    fn an_empty_store_is_a_session_that_says_it_has_no_key() {
        assert_eq!(
            session(Scripted::default()).status().key,
            Key::Absent,
            "a missing key is a state to render, not a failure to open"
        );
    }

    #[test]
    fn a_stored_key_reaches_both_the_status_and_the_model() {
        let mut store = InMemory::default();
        store.set("local", "sk-already-there").unwrap();
        let session = Session::with_model(&config(), Keys::with_override(store, None), |_, key| {
            let mut model = Scripted::default();
            model.use_key(key);
            model
        })
        .unwrap();

        assert_eq!(session.status().key, Key::Present);
        assert_eq!(session.model().key(), Some("sk-already-there"));
    }

    /// A machine with no secret service: a container, a headless Linux box, a
    /// CI runner. The keyring is not merely empty, it will not answer at all.
    #[derive(Debug)]
    struct Unplugged;

    impl KeyStore for Unplugged {
        fn get(&self, _: &str) -> Result<Option<String>> {
            Err(anyhow::anyhow!("no default store has been set"))
        }

        fn set(&mut self, _: &str, _: &str) -> Result<()> {
            Err(anyhow::anyhow!("no default store has been set"))
        }
    }

    #[test]
    fn a_machine_with_no_keyring_still_opens_a_session() {
        // This is what makes "point the config at Ollama and keep chatting"
        // true on a box with no secret service. A local model wants no key, and
        // a client that would not start for want of a keyring it never needed
        // has mistaken its own plumbing for the user's problem.
        let mut session =
            Session::with_model(&config(), Keys::with_override(Unplugged, None), |_, key| {
                let mut model = Scripted::saying(["Hello, I'm Epik."]);
                model.use_key(key);
                model
            })
            .expect("an unreachable keyring is not a reason to refuse a session");

        let Key::Unreachable { reason } = &session.status().key else {
            panic!("the session should say the keyring could not be reached");
        };
        assert!(reason.contains("no default store"), "{reason}");
        assert!(
            session.status().key.wanted(),
            "and should still offer somewhere to put one"
        );

        session
            .send("Hi", &mut Silent, &StopToken::new())
            .expect("and the chat proceeds regardless");
    }

    #[test]
    fn a_keyring_that_will_not_take_a_key_says_so_rather_than_pretending() {
        let mut session =
            Session::with_model(&config(), Keys::with_override(Unplugged, None), |_, _| {
                Scripted::default()
            })
            .unwrap();

        let error = session
            .set_key("sk-pasted")
            .expect_err("a store that cannot keep a key must not claim to have");

        assert!(format!("{error:#}").contains("no default store"));
    }

    #[test]
    fn the_config_s_system_prompt_heads_the_transcript_on_the_wire() {
        let mut session = session(Scripted::saying(["hi"]));

        session.send("Hi", &mut Silent, &StopToken::new()).unwrap();

        assert_eq!(
            session.model().seen()[0][0],
            Message::system(PERSONA),
            "the session opened on the persona the config states"
        );
    }

    #[test]
    fn a_pasted_key_takes_effect_without_the_transcript_being_lost() {
        let mut session = session(Scripted::saying(["one"]).then_saying(["two"]));
        session
            .send("first", &mut Silent, &StopToken::new())
            .unwrap();

        let status = session.set_key("sk-pasted").expect("the store takes it");

        assert_eq!(
            status.key,
            Key::Present,
            "the card has served its purpose and can go"
        );
        assert_eq!(
            session.model().key(),
            Some("sk-pasted"),
            "the key reached the model, not only the store"
        );
        assert_eq!(
            session.messages().len(),
            2,
            "what was already said stands: this is the same conversation"
        );

        // And the next turn goes out on that same conversation.
        session
            .send("second", &mut Silent, &StopToken::new())
            .expect("the chat proceeds without a restart");
        assert_eq!(session.messages().len(), 4);
    }

    #[test]
    fn an_override_in_force_still_outranks_a_pasted_key() {
        let mut session = Session::with_model(
            &config(),
            Keys::with_override(
                InMemory::default(),
                Some("sk-from-the-environment".to_owned()),
            ),
            |_, key| {
                let mut model = Scripted::default();
                model.use_key(key);
                model
            },
        )
        .unwrap();

        session.set_key("sk-pasted").unwrap();

        assert_eq!(
            session.model().key(),
            Some("sk-from-the-environment"),
            "the session uses what resolves, not what was typed"
        );
    }

    #[test]
    fn a_config_naming_an_unlisted_provider_fails_to_open() {
        let config = Config {
            active: "nowhere".to_owned(),
            ..config()
        };

        let error = Session::with_model(
            &config,
            Keys::with_override(InMemory::default(), None),
            |_, _| Scripted::default(),
        )
        .expect_err("there is no provider to open on");

        assert!(error.to_string().contains("nowhere"));
    }
}
