//! The Tauri side of the window: the only place in this crate that knows it is
//! running inside one.
//!
//! Tauri's own JavaScript is reached through the global it installs when
//! `withGlobalTauri` is set — which is how a Rust frontend calls a Rust host
//! with no JavaScript build step anywhere in between.
//!
//! Values cross as JSON text rather than through a JS-value serializer, because
//! the far side of this boundary is `serde_json` either way and one
//! representation is easier to reason about than two.

// A future that awaits JavaScript is never `Send`: the web has one thread, and
// `wasm_bindgen`'s own futures say so. Nothing here is ever moved off it.
#![allow(clippy::future_not_send)]

use epik::event::{ChatEvent, Envelope};
use epik::ipc;
use epik::session::Status;
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(command: &str, args: JsValue) -> Result<JsValue, JsValue>;

    // Not declared `async`: the returned promise is the unlisten function, and
    // this window listens for as long as it exists. Registration proceeds on
    // its own.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], js_name = listen, catch)]
    fn tauri_listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>)
    -> Result<JsValue, JsValue>;
}

/// Tauri's `invoke` wants an object even from a command that takes nothing.
#[derive(Serialize)]
struct Nothing {}

#[derive(Serialize)]
struct Text<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct Key<'a> {
    key: &'a str,
}

#[derive(Serialize)]
struct Token<'a> {
    token: &'a str,
}

/// Who is answering, and whether there is a key to reach them with.
///
/// # Errors
///
/// Returns the host's own words when the session could not be opened.
pub async fn status() -> Result<Status, String> {
    call(ipc::command::STATUS, &Nothing {}).await
}

/// Sends one user turn. Answers when the turn is over, or at once if the host
/// would not start it — the reply itself arrives on the event channel.
///
/// # Errors
///
/// Returns the host's own words when the turn could not be started.
pub async fn send_message(text: &str) -> Result<(), String> {
    call(ipc::command::SEND_MESSAGE, &Text { text }).await
}

/// Files a key for the active provider and puts it into use, answering with the
/// session as it now stands.
///
/// # Errors
///
/// Returns the host's own words when the key could not be stored.
pub async fn set_api_key(key: &str) -> Result<Status, String> {
    call(ipc::command::SET_API_KEY, &Key { key }).await
}

/// Files the GitHub token on its own rails and puts it into use, answering
/// with the session as it now stands.
///
/// # Errors
///
/// Returns the host's own words when the paste cannot serve or could not be
/// stored.
pub async fn set_github_token(token: &str) -> Result<Status, String> {
    call(ipc::command::SET_GITHUB_TOKEN, &Token { token }).await
}

/// Calls `saw` for every chat event the host emits, for as long as the window
/// exists.
///
/// Events this build cannot decode are dropped rather than surfaced: a host
/// further along than the frontend is a thing to survive, not to report.
pub fn listen(mut saw: impl FnMut(Envelope<ChatEvent>) + 'static) {
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |raw: JsValue| {
        if let Some(envelope) = payload(&raw).as_ref().and_then(crate::ipc::chat_event) {
            saw(envelope);
        }
    });
    // A failure to subscribe leaves a window that shows nothing streaming,
    // which the transcript will make obvious; there is nowhere better to say so
    // from here.
    let _ = tauri_listen(ipc::CHANNEL, &handler);
    // The subscription outlives this call, so the closure has to as well.
    handler.forget();
}

/// Invokes `command` with `args` and decodes its answer.
async fn call<A: Serialize, T: DeserializeOwned>(command: &str, args: &A) -> Result<T, String> {
    let outgoing =
        serde_json::to_string(args).map_err(|error| format!("preparing {command}: {error}"))?;
    let outgoing = js_sys::JSON::parse(&outgoing).map_err(|error| complaint(&error))?;

    let answer = tauri_invoke(command, outgoing)
        .await
        .map_err(|error| complaint(&error))?;

    // A command answering with nothing gives `undefined`, which is not JSON.
    // `null` is, and it deserializes to the unit type.
    let incoming = js_sys::JSON::stringify(&answer)
        .ok()
        .and_then(|json| json.as_string())
        .unwrap_or_else(|| "null".to_owned());
    serde_json::from_str(&incoming)
        .map_err(|error| format!("reading what {command} answered: {error}"))
}

/// The `payload` field of one Tauri event, as JSON.
fn payload(raw: &JsValue) -> Option<serde_json::Value> {
    let json = js_sys::JSON::stringify(raw).ok()?.as_string()?;
    let mut event: serde_json::Value = serde_json::from_str(&json).ok()?;
    Some(event.get_mut("payload")?.take())
}

/// What a rejected command said. Every command in this host answers with a
/// string, so the usual case is that string verbatim.
fn complaint(error: &JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| format!("the window could not reach Epik: {error:?}"))
}
