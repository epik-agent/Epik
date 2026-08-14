//! This side of the IPC barrier, in one place: the invoke binding, the
//! command names, and typed calls over them.
//!
//! The types on the wire are the library's own — [`Resolved`] in,
//! [`Secret`] out — so nothing here re-declares what `epik` already says.

use epik::keystore::{Resolved, Secret};
use leptos::task::spawn_local;
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
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
