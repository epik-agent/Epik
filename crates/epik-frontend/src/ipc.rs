//! This side of the IPC barrier, in one place: the invoke and listen
//! bindings, the command names, and typed calls over them.
//!
//! The types on the wire are the library's own — [`Resolved`],
//! [`Secret`], [`TranscriptItem`] — so nothing here re-declares what
//! `epik` already says.

use epik::chat::{TRANSCRIPT_EVENT, TranscriptItem};
use epik::keystore::{Resolved, Secret};
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

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
struct SendArgs<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct OpenUrlArgs<'a> {
    url: &'a str,
}

/// What the event binding hands the callback; the item rides in `payload`.
#[derive(Deserialize)]
struct TranscriptEnvelope {
    payload: TranscriptItem,
}

/// A rejected invoke as text for the page, with a fallback that can never
/// carry secret bytes.
fn error_text(error: &JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "the backend would not answer".to_owned())
}

/// Where the secret filed under `name` stands, by asking the backend.
pub(crate) async fn reveal_secret(name: &str) -> Resolved {
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
pub(crate) async fn save_secret(name: &str, value: &Secret) -> Result<(), String> {
    let args = serde_wasm_bindgen::to_value(&SaveArgs { name, value })
        .map_err(|_| "the request could not be encoded".to_owned())?;
    invoke("secret_save", args)
        .await
        .map(|_| ())
        .map_err(|error| error_text(&error))
}

/// Posts the user's message. Fire-and-forget: the message becomes visible
/// when it comes back around as a transcript event, not before.
pub(crate) fn send_message(text: String) {
    spawn_local(async move {
        let Ok(args) = serde_wasm_bindgen::to_value(&SendArgs { text: &text }) else {
            return;
        };
        let _ = invoke("send_message", args).await;
    });
}

/// The whole transcript so far, for a window that has just mounted.
pub(crate) async fn get_transcript() -> Vec<TranscriptItem> {
    match invoke("get_transcript", JsValue::UNDEFINED).await {
        Ok(items) => serde_wasm_bindgen::from_value(items).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Hands every arriving transcript item to `fold`, for the lifetime of the
/// window — which is why the closure is forgotten rather than dropped.
pub(crate) fn listen_transcript(fold: impl Fn(TranscriptItem) + 'static) {
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

/// Opens `url` in the system browser through the opener plugin — the app's
/// webview is never a place to browse the web.
pub(crate) fn open_url(url: String) {
    spawn_local(async move {
        let Ok(args) = serde_wasm_bindgen::to_value(&OpenUrlArgs { url: &url }) else {
            return;
        };
        let _ = invoke("plugin:opener|open_url", args).await;
    });
}

/// Asks the backend for the settings window. The window is the backend's
/// to make, so this only asks — and fire-and-forget is the right shape for
/// a keystroke.
pub(crate) fn open_settings() {
    spawn_local(async {
        let _ = invoke("settings_open", JsValue::UNDEFINED).await;
    });
}

/// Asks the backend to close the settings window, the same way.
pub(crate) fn close_settings() {
    spawn_local(async {
        let _ = invoke("settings_close", JsValue::UNDEFINED).await;
    });
}
