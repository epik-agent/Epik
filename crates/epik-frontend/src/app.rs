use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

#[component]
pub fn App() -> impl IntoView {
    // Cmd+, is the platform convention on macOS, Ctrl+, elsewhere; accepting
    // either modifier serves both without asking the OS which one this is.
    // The window is the backend's to make, so this only asks.
    let handle = window_event_listener(ev::keydown, move |event| {
        if (event.meta_key() || event.ctrl_key()) && event.key() == "," {
            event.prevent_default();
            spawn_local(async {
                let _ = invoke("settings_open", JsValue::UNDEFINED).await;
            });
        }
    });
    on_cleanup(move || handle.remove());

    view! {
        <main class="flex h-screen items-center justify-center">
            <h1 class="text-2xl font-semibold text-gray-400">"Epik"</h1>
        </main>
    }
}
