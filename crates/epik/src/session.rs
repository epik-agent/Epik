//! One client's chat session: the config read, the key resolved, the model
//! built, the transcript kept.
//!
//! This module exists because of the invariant: anything the app can do must
//! be achievable through calls into this crate alone. Without it, the wiring
//! between [`Config`], [`Keys`] and a [`ChatModel`] would live in the Tauri
//! host, and the daemon would have to reinvent it. With it, a host is a
//! handful of commands over as many library calls, and every future thin
//! client opens a chat the same way.
//!
//! [`Status`] is the part that crosses IPC: what the status bar names, and
//! whether there is a key to reach it with. Never the key itself.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::chat::{ChatModel, Conversation, Keyed, Message, Reply, StopToken, ToolDeclaration};
use crate::config::{Config, Provider};
use crate::event::ChatEvent;
use crate::keystore::{Credential, KeyStore, Keys, Resolved};
use crate::logging::Log;
use crate::tools::Registry;

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

/// Where resolution got to, as a client is allowed to know it.
impl From<&Resolved> for Key {
    fn from(resolved: &Resolved) -> Self {
        match resolved {
            Resolved::Found(_) => Self::Present,
            Resolved::Absent => Self::Absent,
            Resolved::Unreachable(reason) => Self::Unreachable {
                reason: reason.clone(),
            },
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
    /// Where the GitHub token stands, on its own rails. This is what puts
    /// the paste-your-PAT card away once a pasted token has been kept.
    pub github: Key,
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
    tools: Registry,
    /// The GitHub client's credential, shared with the verbs in `tools`:
    /// [`set_github_token`](Self::set_github_token) writes here and every
    /// registered verb reads it. A session built without GitHub — a test on
    /// a scripted model — holds an unshared cell, and writing to it is
    /// simply true and unread.
    github: Credential,
}

impl<M: ChatModel + Keyed, S: KeyStore> Session<M, S> {
    /// A session on the active provider, with `tools` for the model to work
    /// through and `build` making the model that speaks to it.
    ///
    /// `build` is handed the tools' declarations so the model can be
    /// constructed already offering them — a model with no way to offer
    /// tools, like the scripted one in a test, simply ignores them. An empty
    /// registry is a session that chats and nothing else.
    ///
    /// [`open`](Session::open) is this with the OpenAI-compatible client and
    /// the GitHub verbs. A native Anthropic client would arrive the same
    /// way, and so does the scripted model in a test — which is the point of
    /// taking the constructor rather than naming a type.
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
        tools: Registry,
        build: impl FnOnce(&Provider, Option<String>, Vec<ToolDeclaration>) -> M,
    ) -> Result<Self> {
        let github = keys.github_token();
        Self::with_github(config, keys, &github, tools, build)
    }

    /// [`with_model`](Session::with_model) with the GitHub token already
    /// resolved. This is the seam that keeps launch to one keyring read per
    /// secret: [`open`](Session::open) needs the token before the session
    /// exists — its GitHub client is built first — so it resolves once and
    /// hands the result here rather than having [`Status`] ask again. On
    /// macOS every read is potentially a dialog, which makes the count part
    /// of the interface rather than a nicety.
    fn with_github(
        config: &Config,
        keys: Keys<S>,
        github: &Resolved,
        tools: Registry,
        build: impl FnOnce(&Provider, Option<String>, Vec<ToolDeclaration>) -> M,
    ) -> Result<Self> {
        let (name, provider) = config.provider()?;
        let resolved = keys.resolve(name);
        let status = Status {
            provider: name.to_owned(),
            model: provider.model.clone(),
            key: (&resolved).into(),
            github: github.into(),
        };
        let model = build(provider, resolved.key(), tools.declarations());
        Ok(Self {
            status,
            conversation: Conversation::new(config.system_prompt.clone(), model),
            keys,
            tools,
            github: Credential::default(),
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

    /// One turn, run against the session's tools: see
    /// [`Conversation::send_with_tools`]. A model that answers in plain text
    /// makes it one round, exactly as a plain send; one that asks for tools
    /// gets them run and the results resent until it answers.
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
        self.conversation
            .send_with_tools(text, &mut self.tools, log, stop)
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

    /// Files the GitHub token and puts it into use at once:
    /// [`set_key`](Self::set_key)'s sibling, and what the paste-your-PAT
    /// card calls. The verbs already registered read the token through the
    /// shared credential, so the very next call succeeds with no restart.
    ///
    /// The token is read back through resolution rather than used as
    /// pasted, for [`set_key`](Self::set_key)'s reason: an
    /// `EPIK_GITHUB_TOKEN` override outranks the store and keeps winning
    /// for this process, while the paste is kept for the next one.
    ///
    /// A store that will not take the token does not un-take the paste. The
    /// token is in hand, so it is used now and simply not kept — which is
    /// what the card's warning promised on exactly that machine — and the
    /// next launch resolves to wherever things truly stand.
    ///
    /// # Errors
    ///
    /// Turns away a paste that is certain to strand the user — a GitHub
    /// App's hour-long token, a chat key in the wrong box — before anything
    /// is stored.
    pub fn set_github_token(&mut self, token: &str) -> Result<&Status> {
        if let Some(reason) = unfit(token) {
            anyhow::bail!("{reason}");
        }
        // The write's failure is not raised: the card warned it would not
        // be kept, and refusing to *use* what the user just handed over
        // would make the card a dead end on a machine with no keyring.
        let kept = self.keys.set_github_token(token);
        let resolved = match self.keys.github_token() {
            Resolved::Unreachable(_) if kept.is_err() => Resolved::Found(token.to_owned()),
            resolved => resolved,
        };
        self.status.github = (&resolved).into();
        self.github.use_token(resolved.key());
        Ok(&self.status)
    }
}

/// Why a pasted token cannot serve, when it cannot.
///
/// Only certainties are turned away, each with the sentence the card shows.
/// An unrecognized prefix passes: GitHub has minted new shapes before, and
/// refusing a working token is worse than keeping a dud — a dud refuses
/// loudly on its first call anyway.
fn unfit(token: &str) -> Option<&'static str> {
    if token.starts_with("ghs_") || token.starts_with("ghu_") {
        Some(
            "that is a GitHub App token, which expires within the hour — \
             paste a personal access token instead",
        )
    } else if token.starts_with("sk-") {
        Some("that is a chat-provider key, not a GitHub token — it belongs on the other card")
    } else {
        None
    }
}

#[cfg(feature = "native")]
impl<S: KeyStore> Session<crate::chat::OpenAiCompatible, S> {
    /// A session on the active provider, over the OpenAI chat-completions
    /// protocol, with the GitHub verbs registered as the model's tools: the
    /// whole of what starting a chat takes.
    ///
    /// The GitHub token rides the keystore rails — `EPIK_GITHUB_TOKEN`, then
    /// the keyring, then absent — and its absence opens the session anyway:
    /// the registry builds tokenless, and the verbs that need one refuse at
    /// call time with a typed refusal the model reads as a tool result.
    ///
    /// Each secret is read from the store once: the token resolved for the
    /// GitHub client is the same resolution [`Status`] reports. On macOS a
    /// keyring read can be a permission dialog, so launch asks each question
    /// exactly one time.
    ///
    /// # Errors
    ///
    /// As [`with_model`](Session::with_model).
    pub fn open(config: &Config, keys: Keys<S>) -> Result<Self> {
        let token = keys.github_token();
        let github = crate::github::GitHub::new(token.clone().key());
        let credential = github.credential();
        let mut tools = Registry::new();
        crate::github::tools::register(&mut tools, github);
        let mut session = Self::with_github(
            config,
            keys,
            &token,
            tools,
            |provider, key, declarations| {
                let mut model = crate::chat::OpenAiCompatible::new(provider, key);
                model.use_tools(declarations);
                model
            },
        )?;
        // The same cell the registered verbs read: this is what makes
        // `set_github_token` reach them.
        session.github = credential;
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::chat::Scripted;
    use crate::keystore::{InMemory, Unplugged};
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
            worker: crate::config::Worker::default(),
        }
    }

    /// A session on the scripted model, with an empty store, no override,
    /// and no tools.
    fn session(model: Scripted) -> Session<Scripted, InMemory> {
        Session::with_model(
            &config(),
            Keys::with_override(InMemory::default(), None),
            Registry::new(),
            |_, key, _| {
                let mut model = model;
                model.use_key(key);
                model
            },
        )
        .expect("the config names a provider it lists")
    }

    #[cfg(feature = "native")]
    #[test]
    fn open_reads_each_secret_from_the_store_exactly_once() {
        use crate::keystore::{Counting, GITHUB_ACCOUNT};

        let mut store = Counting::default();
        store.set("local", "sk-kept").unwrap();
        store.set(GITHUB_ACCOUNT, "ghp-kept").unwrap();
        let reads = store.reads();

        Session::open(&config(), Keys::with_overrides(store, None, None))
            .expect("the config names a provider it lists");

        assert_eq!(
            *reads.borrow(),
            BTreeMap::from([(GITHUB_ACCOUNT.to_owned(), 1), ("local".to_owned(), 1)]),
            "on macOS every keyring read is potentially a dialog, so launch \
             asks each question once"
        );
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
        let session = Session::with_model(
            &config(),
            Keys::with_override(store, None),
            Registry::new(),
            |_, key, _| {
                let mut model = Scripted::default();
                model.use_key(key);
                model
            },
        )
        .unwrap();

        assert_eq!(session.status().key, Key::Present);
        assert_eq!(session.model().key(), Some("sk-already-there"));
    }

    #[test]
    fn a_machine_with_no_keyring_still_opens_a_session() {
        // This is what makes "point the config at Ollama and keep chatting"
        // true on a box with no secret service. A local model wants no key, and
        // a client that would not start for want of a keyring it never needed
        // has mistaken its own plumbing for the user's problem.
        let mut session = Session::with_model(
            &config(),
            Keys::with_override(Unplugged, None),
            Registry::new(),
            |_, key, _| {
                let mut model = Scripted::saying(["Hello, I'm Epik."]);
                model.use_key(key);
                model
            },
        )
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
        let mut session = Session::with_model(
            &config(),
            Keys::with_override(Unplugged, None),
            Registry::new(),
            |_, _, _| Scripted::default(),
        )
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
            Registry::new(),
            |_, key, _| {
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
    fn a_paste_onto_a_machine_with_no_keyring_is_used_now_and_not_kept() {
        // The card's warning on that machine says exactly this: the token
        // will not be kept — not that pasting is futile. A dead-end card on
        // the one machine that warns about itself would be worse than none.
        let mut session = Session::with_model(
            &config(),
            Keys::with_override(Unplugged, None),
            Registry::new(),
            |_, _, _| Scripted::default(),
        )
        .unwrap();
        assert!(matches!(session.status().github, Key::Unreachable { .. }));

        let status = session
            .set_github_token("ghp-pasted")
            .expect("a store that will not keep the token does not un-take the paste");

        assert_eq!(
            status.github,
            Key::Present,
            "in use is the truth of this session"
        );
        assert_eq!(
            session.github.token().as_deref(),
            Some("ghp-pasted"),
            "the credential is armed from the hand, not the store"
        );
    }

    #[test]
    fn a_pasted_github_token_arms_the_credential_and_flips_the_status() {
        let mut session = session(Scripted::default());
        assert_eq!(
            session.status().github,
            Key::Absent,
            "a fresh store has no token, and that is a state, not a fault"
        );

        let status = session
            .set_github_token("ghp-pasted")
            .expect("the store takes it");

        assert_eq!(status.github, Key::Present, "the card can go");
        assert_eq!(
            session.github.token().as_deref(),
            Some("ghp-pasted"),
            "the credential is armed: every verb sharing it sees the token now"
        );
    }

    #[test]
    fn a_github_override_in_force_still_outranks_a_pasted_token() {
        let mut session = Session::with_model(
            &config(),
            Keys::with_overrides(
                InMemory::default(),
                None,
                Some("ghp-from-the-environment".to_owned()),
            ),
            Registry::new(),
            |_, _, _| Scripted::default(),
        )
        .unwrap();

        session.set_github_token("ghp-pasted").unwrap();

        assert_eq!(
            session.github.token().as_deref(),
            Some("ghp-from-the-environment"),
            "the session uses what resolves, not what was typed"
        );
    }

    #[test]
    fn a_paste_that_cannot_serve_is_turned_away_before_it_is_stored() {
        let mut session = session(Scripted::default());

        for (token, tell) in [
            ("ghs_installation", "GitHub App token"),
            ("ghu_user_to_server", "GitHub App token"),
            ("sk-chat-key", "chat-provider key"),
        ] {
            let error = session
                .set_github_token(token)
                .expect_err("a paste certain to strand the user is refused");
            assert!(format!("{error:#}").contains(tell), "{token}: {error:#}");
            assert_eq!(
                session.status().github,
                Key::Absent,
                "{token}: nothing was stored"
            );
        }
    }

    #[test]
    fn an_unrecognized_prefix_is_kept_rather_than_second_guessed() {
        // GitHub has minted new token shapes before; only certainties are
        // turned away. `github_pat_` and `gho_` are the shapes the card's
        // own guidance produces.
        for token in [
            "github_pat_fine_grained",
            "ghp_classic",
            "gho_from_gh",
            "odd",
        ] {
            let mut session = session(Scripted::default());
            session
                .set_github_token(token)
                .expect("an unfamiliar shape is not a refusal");
            assert_eq!(session.status().github, Key::Present, "{token}");
        }
    }

    #[test]
    fn a_turn_runs_through_the_sessions_tools() {
        // The window's whole turn goes through here, so this is the seam
        // that makes the vertical slice true: a model that asks for a tool
        // gets the result resent, all inside one Session::send.
        use crate::chat::ToolCall;
        use crate::tools::Tool;

        let mut tools = Registry::new();
        tools.register(
            Tool::new("echo", "Says the arguments back.", serde_json::json!({})),
            |args: serde_json::Value| Ok::<_, std::convert::Infallible>(args),
        );
        let scripted = Scripted::saying(["Checking."])
            .asking([ToolCall::new("call-1", "echo", r#"{"n":1}"#)])
            .then_saying(["Done."]);
        let mut session = Session::with_model(
            &config(),
            Keys::with_override(InMemory::default(), None),
            tools,
            |_, _, _| scripted,
        )
        .unwrap();

        let reply = session.send("Go", &mut Silent, &StopToken::new()).unwrap();

        assert_eq!(reply.text, "Done.");
        assert!(
            session
                .messages()
                .contains(&Message::tool_result("call-1", r#"{"n":1}"#)),
            "the tool's answer is in the canonical transcript: {:?}",
            session.messages()
        );
    }

    #[test]
    fn the_tools_declarations_reach_the_model_builder() {
        use crate::tools::Tool;

        let mut tools = Registry::new();
        tools.register(
            Tool::new("echo", "Says the arguments back.", serde_json::json!({})),
            |args: serde_json::Value| Ok::<_, std::convert::Infallible>(args),
        );
        let mut offered = Vec::new();
        Session::with_model(
            &config(),
            Keys::with_override(InMemory::default(), None),
            tools,
            |_, _, declarations| {
                offered = declarations;
                Scripted::default()
            },
        )
        .unwrap();

        assert_eq!(offered.len(), 1);
        assert_eq!(
            offered[0].name, "echo",
            "the model is built already offering what the registry holds"
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
            Registry::new(),
            |_, _, _| Scripted::default(),
        )
        .expect_err("there is no provider to open on");

        assert!(error.to_string().contains("nowhere"));
    }
}
