//! This side of the IPC barrier, in one place: the invoke and listen
//! bindings, the command names, and typed calls over them.
//!
//! The types on the wire are the library's own — [`Resolved`],
//! [`Secret`], [`Config`], [`ModelInfo`], [`TranscriptItem`], [`Entry`]
//! — so nothing here re-declares what `epik` already says.

use epik::chat::{Answer, ModelInfo, TRANSCRIPT_EVENT, TranscriptItem};
use epik::config::Config;
use epik::keystore::{Resolved, Secret};
use epik::monitor::{EVENT, Entry};
use leptos::prelude::window;
use leptos::task::spawn_local;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::monitor::Feed;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "event"])]
    async fn listen(event: &str, handler: &JsValue) -> Result<JsValue, JsValue>;
}

#[derive(Serialize)]
struct RevealArgs<'a> {
    name: &'a str,
}

#[derive(Serialize)]
struct SaveArgs<'a> {
    name: &'a str,
    value: &'a Secret,
}

#[derive(Serialize)]
struct ConfigArgs<'a> {
    config: &'a Config,
}

#[derive(Serialize)]
struct SendArgs<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct OpenUrlArgs<'a> {
    url: &'a str,
}

#[derive(Serialize)]
struct ThemeArgs<'a> {
    theme: &'a str,
}

#[derive(Serialize)]
struct AnswerArgs<'a> {
    id: &'a str,
    answer: &'a Answer,
}

/// The dialog plugin's save-dialog request: `{ options }`, in the
/// plugin's own camelCase.
#[derive(Serialize)]
struct SaveDialogArgs<'a> {
    options: SaveDialogOptions<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveDialogOptions<'a> {
    title: &'a str,
    default_path: &'a str,
    can_create_directories: bool,
}

/// What the event binding hands the callback; the item rides in `payload`.
#[derive(Deserialize)]
struct TranscriptEnvelope {
    payload: TranscriptItem,
}

/// The same envelope on the monitor's channel; the entry rides in
/// `payload`.
#[derive(Deserialize)]
struct MonitorEnvelope {
    payload: Entry,
}

/// A rejected invoke as text for the page, with a fallback that can never
/// carry secret bytes.
fn error_text(error: &JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "the backend would not answer".to_owned())
}

/// A command that answers with a value: the Err is the channel's
/// refusal, or an answer the page cannot read.
async fn ask<T: DeserializeOwned>(command: &str) -> Result<T, String> {
    let outcome = invoke(command, JsValue::UNDEFINED)
        .await
        .map_err(|error| error_text(&error))?;
    serde_wasm_bindgen::from_value(outcome)
        .map_err(|_| "an unintelligible answer from the backend".to_owned())
}

/// A command that answers with nothing: `args` encoded, sent, and the
/// Err is the channel's refusal.
async fn tell(command: &str, args: &impl Serialize) -> Result<(), String> {
    let args = serde_wasm_bindgen::to_value(args)
        .map_err(|_| "the request could not be encoded".to_owned())?;
    invoke(command, args)
        .await
        .map(|_| ())
        .map_err(|error| error_text(&error))
}

/// Where the secret filed under `name` stands, by asking the backend.
pub async fn reveal_secret(name: &str) -> Resolved {
    let Ok(args) = serde_wasm_bindgen::to_value(&RevealArgs { name }) else {
        return Resolved::Unreachable("the request could not be encoded".to_owned());
    };
    match invoke("secret_reveal", args).await {
        Ok(outcome) => serde_wasm_bindgen::from_value(outcome).unwrap_or_else(|_| {
            Resolved::Unreachable("an unintelligible answer from the backend".to_owned())
        }),
        Err(error) => Resolved::Unreachable(error_text(&error)),
    }
}

/// Files `value` under `name`, through the backend.
pub async fn save_secret(name: &str, value: &Secret) -> Result<(), String> {
    tell("secret_save", &SaveArgs { name, value }).await
}

/// The backend's live configuration, as the file states it. The Err is
/// a channel that would not answer — the file itself was read at startup.
pub async fn read_config() -> Result<Config, String> {
    ask("config_read").await
}

/// Writes `config` to the file and makes it the backend's live
/// configuration, in one command.
pub async fn write_config(config: &Config) -> Result<(), String> {
    tell("config_write", &ConfigArgs { config }).await
}

/// The models the provider will answer for, newest first. The backend
/// reads the key from the keyring itself; the Err is why there is no list
/// — no key yet, or the provider's own words.
pub async fn list_models() -> Result<Vec<ModelInfo>, String> {
    ask("models_list").await
}

/// The login of the account the stored GitHub token belongs to.
pub async fn github_login() -> Result<String, String> {
    ask("github_login").await
}

/// Posts the user's message. The message becomes visible when it comes
/// back around as a transcript event; the Err is the command channel's
/// refusal, distinct from a turn that fails.
pub async fn send_message(text: String) -> Result<(), String> {
    tell("send_message", &SendArgs { text: &text }).await
}

/// Answers the pending question `id`. The card that asked shows nothing
/// on its own account: the resolution comes back around as a
/// `QuestionResolved` event. The Err is the command channel's refusal —
/// a stale card, a double answer.
pub async fn answer_question(id: &str, answer: &Answer) -> Result<(), String> {
    tell("answer_question", &AnswerArgs { id, answer }).await
}

/// The default name the Browse dialog offers for a repository-to-be.
pub const DEFAULT_REPOSITORY_NAME: &str = "repository.git";

/// Opens the native save dialog to name a repository directory that need
/// not exist yet, and returns the chosen path — `None` when the user
/// cancels, or when the dialog could not be opened at all.
pub async fn browse_repository() -> Option<String> {
    let args = serde_wasm_bindgen::to_value(&SaveDialogArgs {
        options: SaveDialogOptions {
            title: "Where should the repository live?",
            default_path: DEFAULT_REPOSITORY_NAME,
            can_create_directories: true,
        },
    })
    .ok()?;
    invoke("plugin:dialog|save", args).await.ok()?.as_string()
}

/// The whole transcript so far, for a window that has just mounted.
pub async fn get_transcript() -> Vec<TranscriptItem> {
    match invoke("get_transcript", JsValue::UNDEFINED).await {
        Ok(items) => serde_wasm_bindgen::from_value(items).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Hands every arriving transcript item to `fold`, for the lifetime of the
/// window — which is why the closure is forgotten rather than dropped.
pub fn listen_transcript(fold: impl Fn(TranscriptItem) + 'static) {
    spawn_local(async move {
        let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            if let Ok(envelope) = serde_wasm_bindgen::from_value::<TranscriptEnvelope>(event) {
                fold(envelope.payload);
            }
        });
        let _ = listen(TRANSCRIPT_EVENT, handler.as_ref()).await;
        handler.forget();
    });
}

/// The window's feed of feature-build entries: each one as an event on
/// [`EVENT`], the replay from the `monitor_log` command.
pub struct Monitor;

impl Feed for Monitor {
    /// The handler lives for the window, which is why it is forgotten
    /// rather than dropped; `standing` hears whether the binding took,
    /// once — the window's channel is neither lost nor regained.
    fn listen(
        &self,
        hear: impl Fn(Entry) + 'static,
        standing: impl Fn(Result<(), String>) + 'static,
    ) {
        spawn_local(async move {
            let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                if let Ok(envelope) = serde_wasm_bindgen::from_value::<MonitorEnvelope>(event) {
                    hear(envelope.payload);
                }
            });
            let outcome = listen(EVENT, handler.as_ref())
                .await
                .map(|_| ())
                .map_err(|error| error_text(&error));
            handler.forget();
            standing(outcome);
        });
    }

    fn replay(&self, deliver: impl FnOnce(Result<Vec<Entry>, String>) + 'static) {
        spawn_local(async move { deliver(ask("monitor_log").await) });
    }
}

/// Whether the page runs in the app's window: Tauri's bridge is on
/// `window` there, and nowhere else.
pub fn in_window() -> bool {
    js_sys::Reflect::has(&window(), &JsValue::from_str("__TAURI__")).unwrap_or(false)
}

/// Opens `url` in the system browser through the opener plugin — the app's
/// webview is never a place to browse the web.
pub fn open_url(url: String) {
    spawn_local(async move {
        let Ok(args) = serde_wasm_bindgen::to_value(&OpenUrlArgs { url: &url }) else {
            return;
        };
        let _ = invoke("plugin:opener|open_url", args).await;
    });
}

/// Asks the backend to dress every window in `theme` — "light" or "dark".
pub fn set_theme(theme: &'static str) {
    spawn_local(async move {
        let Ok(args) = serde_wasm_bindgen::to_value(&ThemeArgs { theme }) else {
            return;
        };
        let _ = invoke("set_theme", args).await;
    });
}

/// Asks the backend for the settings window. The window is the backend's
/// to make, so this only asks — and fire-and-forget is the right shape for
/// a keystroke.
pub fn open_settings() {
    spawn_local(async {
        let _ = invoke("settings_open", JsValue::UNDEFINED).await;
    });
}

/// Asks the backend to close the settings window, the same way.
pub fn close_settings() {
    spawn_local(async {
        let _ = invoke("settings_close", JsValue::UNDEFINED).await;
    });
}
