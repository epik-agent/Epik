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
use crate::keystore::{KeyStore, Keys};
use crate::logging::Log;

/// What a client needs to know about the session it is in.
///
/// A plain serde type, because it crosses the IPC boundary: the status bar
/// names `model`, and `has_key` is what decides whether the chat pane shows a
/// paste-your-key card. The key it describes stays where the operating system
/// put it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Status {
    /// The active provider's configured name — which is also the account its
    /// key is filed under.
    pub provider: String,
    /// The model the provider was asked for, verbatim from the config.
    pub model: String,
    /// Whether a key was found. Absent is an ordinary state, not a fault:
    /// local servers want none.
    pub has_key: bool,
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
    /// list, or when the key store could not be consulted. A *missing* key is
    /// not an error: [`Status::has_key`] reports it, and the client decides
    /// what to do about it.
    pub fn with_model(
        config: &Config,
        keys: Keys<S>,
        build: impl FnOnce(&Provider, Option<String>) -> M,
    ) -> Result<Self> {
        let (name, provider) = config.provider()?;
        let key = keys.get(name)?;
        let status = Status {
            provider: name.to_owned(),
            model: provider.model.clone(),
            has_key: key.is_some(),
        };
        let model = build(provider, key);
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
        // session uses has to be whatever actually resolves.
        let resolved = self.keys.get(&self.status.provider)?;
        self.status.has_key = resolved.is_some();
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
        assert!(
            !session(Scripted::default()).status().has_key,
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

        assert!(session.status().has_key);
        assert_eq!(session.model().key(), Some("sk-already-there"));
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

        assert!(status.has_key, "the card has served its purpose and can go");
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
