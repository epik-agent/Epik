//! What the window holds: the session behind a mutex, the commands the
//! frontend calls, and the pump that carries library events to it.
//!
//! There is deliberately no logic here. Each command is one library call
//! wrapped in the two things Tauri needs and the library refuses to know
//! about: a blocking thread to run on, and a serializable error. Everything
//! worth testing is next door in `epik`, tested without a window.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread;

use epik::chat::{OpenAiCompatible, StopToken};
use epik::config::Config;
use epik::event::{ChatEvent, ConversationId, Envelope};
use epik::ipc;
use epik::keystore::{Keys, OsKeyring};
use epik::logging::Enveloping;
use epik::session::{Session, Status};
use tauri::{AppHandle, Emitter, State};

/// The host's registry, at the size v0 needs it. Exactly one conversation
/// exists, so exactly one id is ever handed out — and a `Conversation` still
/// does not learn its own, which is what leaves room for a second one without
/// touching the protocol.
const ONLY: ConversationId = ConversationId(0);

/// A second turn arrived while one was still streaming. A command error, and
/// the reason the frontend disables its input: this is the backstop, not the
/// mechanism.
const BUSY: &str = "one turn at a time: this conversation is still answering";

/// A previous turn panicked while holding the session. Nothing further can be
/// trusted of it, and saying so is better than deadlocking or pretending.
const POISONED: &str = "the session did not survive an earlier failure; restart Epik";

/// The window's one session. Named for what it is at this end of the IPC
/// boundary, where `Session` would be a mouthful of generics.
type Chat = Session<OpenAiCompatible, OsKeyring>;

/// The chat session the window is showing, and the channel it reports on.
#[derive(Debug)]
struct Host {
    /// One in-flight turn per conversation *is* this mutex: a turn that finds
    /// it held is refused rather than queued.
    session: Arc<Mutex<Chat>>,
    /// A `Sender` is a `Log`, so the library streams straight into this and
    /// never hears of Tauri. The `Mutex` is only because a `Sender` is `Send`
    /// but not `Sync`, and managed state has to be both.
    events: Mutex<Sender<Envelope<ChatEvent>>>,
}

impl Host {
    fn open(events: Sender<Envelope<ChatEvent>>) -> anyhow::Result<Self> {
        let config = Config::load()?;
        let session = Session::open(&config, Keys::new(OsKeyring))?;
        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            events: Mutex::new(events),
        })
    }

    /// The session, to be locked on a blocking thread rather than this one.
    fn chat(&self) -> Arc<Mutex<Chat>> {
        Arc::clone(&self.session)
    }

    /// A sender for the turn about to run. Cloning is the point: each turn
    /// gets its own handle, and the channel outlives all of them.
    fn events(&self) -> Result<Sender<Envelope<ChatEvent>>, String> {
        self.events
            .lock()
            .map(|events| events.clone())
            .map_err(|_| POISONED.to_owned())
    }
}

/// The window's state: the host that opened, or the reason it did not.
///
/// A config Epik cannot make sense of must not stop the window from opening —
/// a client that cannot start is worse than one that can say why — so the
/// failure is state, and every command answers with it.
#[derive(Debug)]
pub struct Window(Result<Host, String>);

impl Window {
    /// Opens a session on the configured provider, reporting on `events`.
    #[must_use]
    pub fn open(events: Sender<Envelope<ChatEvent>>) -> Self {
        Self(Host::open(events).map_err(|error| problem(&error)))
    }

    fn host(&self) -> Result<&Host, String> {
        self.0.as_ref().map_err(Clone::clone)
    }
}

/// A library error as the frontend will read it: the whole chain, since the
/// context is usually where the answer is.
fn problem(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

/// Runs `work` against the session on a blocking thread, which is where
/// library calls belong under Tauri: the library blocks by design, and this
/// process's main thread is the webview's.
///
/// This *waits* for the lock rather than refusing it. A question about the
/// session, or a key that has just been pasted, is still wanted when a turn
/// happens to be in flight — where a second turn is not.
async fn on_the_session<T: Send + 'static>(
    session: Arc<Mutex<Chat>>,
    work: impl FnOnce(&mut Chat) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut session = session.lock().map_err(|_| POISONED.to_owned())?;
        work(&mut session)
    })
    .await
    .map_err(|error| format!("the session could not be reached: {error}"))?
}

/// Who is answering: what the status bar names, and whether a key was found.
///
/// A query, not an intent — the frontend asks on mount, and again after
/// storing a key — which is why it stands apart from the two commands that
/// carry intent.
///
/// # Errors
///
/// Returns the reason the session could not be opened, when it could not be.
#[tauri::command]
pub async fn status(window: State<'_, Window>) -> Result<Status, String> {
    let session = window.host()?.chat();
    on_the_session(session, |session| Ok(session.status().clone())).await
}

/// One user turn.
///
/// The reply does not come back here: it arrives as events, and this result
/// says only whether the turn could be *started*. That is the two channels
/// staying separate — a refusal is a command error, a turn that broke
/// mid-stream is a `Failed` event, and neither is ever reported as the other.
///
/// # Errors
///
/// Returns [`BUSY`] when a turn is already streaming, and the reason the
/// session could not be opened when it could not be.
#[tauri::command]
pub async fn send_message(window: State<'_, Window>, text: String) -> Result<(), String> {
    let host = window.host()?;
    let session = host.chat();
    let events = host.events()?;

    tauri::async_runtime::spawn_blocking(move || {
        // `try_lock` rather than `lock` is the whole of "one in-flight turn
        // per conversation". A second question is refused outright, because a
        // queue would answer questions in an order nobody asked for.
        {
            let mut session = match session.try_lock() {
                Ok(session) => session,
                Err(TryLockError::WouldBlock) => return Err(BUSY.to_owned()),
                Err(TryLockError::Poisoned(_)) => return Err(POISONED.to_owned()),
            };
            let mut log = Enveloping::new(ONLY, events);
            // The Stop button is deferred, but the token is already in every
            // signature, so a fresh one per turn is all it will ever need.
            let stop = StopToken::new();
            // The error is dropped on purpose: the same failure has already
            // gone out as a `Failed` event, and returning it here as well
            // would put one turn on both channels.
            let _ = session.send(text, &mut log, &stop);
        }
        // The lock is released before this answers, so the next turn is free
        // to start the moment the frontend re-enables its input.
        Ok(())
    })
    .await
    .map_err(|error| format!("the turn could not be run: {error}"))?
}

/// Files a key for the active provider and puts it into use.
///
/// Answers with the session as it now stands, so the pane can put the
/// paste-your-key card away without asking again — and the chat proceeds on
/// the same transcript, with no restart.
///
/// # Errors
///
/// Returns an error when the keyring would not take the key, and the reason
/// the session could not be opened when it could not be.
#[tauri::command]
pub async fn set_api_key(window: State<'_, Window>, key: String) -> Result<Status, String> {
    let session = window.host()?.chat();
    on_the_session(session, move |session| {
        session
            .set_key(&key)
            .cloned()
            .map_err(|error| problem(&error))
    })
    .await
}

/// Files the GitHub token on its own rails and puts it into use.
///
/// Answers with the session as it now stands, so the pane can put the
/// paste-your-PAT card away — and the verbs already registered see the token
/// on their very next call, with no restart.
///
/// # Errors
///
/// Returns the library's refusal when the paste cannot serve or the keyring
/// would not take it, and the reason the session could not be opened when it
/// could not be.
#[tauri::command]
pub async fn set_github_token(window: State<'_, Window>, token: String) -> Result<Status, String> {
    let session = window.host()?.chat();
    on_the_session(session, move |session| {
        session
            .set_github_token(&token)
            .cloned()
            .map_err(|error| problem(&error))
    })
    .await
}

/// Carries the library's event stream to the webview.
///
/// One thread, one channel, and the only place in Epik that knows both a
/// `ChatEvent` and a webview exist.
pub fn pump(app: AppHandle, events: Receiver<Envelope<ChatEvent>>) {
    thread::spawn(move || {
        for envelope in events {
            // A window that has gone away is not this thread's problem.
            let _ = app.emit(ipc::CHANNEL, &envelope);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `#[tauri::command]` is registered under its function name, and the
    /// frontend calls it by the library's constant. Nothing but this makes the
    /// two agree, so this is the seam where a rename gets caught — at compile
    /// time for the function, here for the name.
    #[test]
    fn every_command_is_registered_under_the_name_the_library_publishes() {
        // Referring to each function is what makes this fail to compile rather
        // than merely fail, if one is renamed or removed.
        let _ = status;
        let _ = send_message;
        let _ = set_api_key;
        let _ = set_github_token;

        assert_eq!(ipc::command::STATUS, "status");
        assert_eq!(ipc::command::SEND_MESSAGE, "send_message");
        assert_eq!(ipc::command::SET_API_KEY, "set_api_key");
        assert_eq!(ipc::command::SET_GITHUB_TOKEN, "set_github_token");
    }
}
