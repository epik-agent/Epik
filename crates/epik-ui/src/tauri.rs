//! Calling the app's Tauri commands, through the `invoke` Tauri puts on the
//! window because `withGlobalTauri` is on.

use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn tauri_invoke(command: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// Runs `command` with `args`, and gives what it returns, or its error.
pub async fn invoke<A: Serialize, R: DeserializeOwned>(
    command: &str,
    args: &A,
) -> Result<R, String> {
    let args = serde_wasm_bindgen::to_value(args).map_err(|error| error.to_string())?;
    match tauri_invoke(command, args).await {
        Ok(value) => serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string()),
        Err(reason) => Err(reason.as_string().unwrap_or_else(|| format!("{reason:?}"))),
    }
}

/// Runs `command` with `args`, ignoring what it returns and any error.
pub fn run<A: Serialize + 'static>(command: &'static str, args: A) {
    leptos::task::spawn_local(async move {
        let _ = invoke::<A, ()>(command, &args).await;
    });
}
