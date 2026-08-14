//! The settings window: a list of labeled secret rows that happens, so far,
//! to have one row — Anthropic.
//!
//! A secret's bytes live in exactly one place on this side: the password
//! input's reactive state. They arrive there through `secret_reveal` and
//! leave through `secret_save`, and nowhere else — never a log line, never
//! a persisted structure.

use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// The keyring entry the one row manages, under the backend's service.
const ANTHROPIC_NAME: &str = "ANTHROPIC_API_KEY";
/// What the row calls it.
const ANTHROPIC_LABEL: &str = "Anthropic";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// The frontend's reading of the backend's `RevealOutcome`. Deliberately not
/// `Debug`: nothing that can hold a secret gets a printable form.
#[derive(Clone, Deserialize)]
enum RevealOutcome {
    Found(String),
    Absent,
    Unreachable(String),
}

#[derive(Serialize)]
struct RevealArgs<'a> {
    name: &'a str,
}

#[derive(Serialize)]
struct SaveArgs<'a> {
    name: &'a str,
    value: &'a str,
}

/// A rejected invoke as text for the page, with a fallback that can never
/// carry secret bytes.
fn error_text(error: &JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "the backend would not answer".to_owned())
}

async fn reveal(name: &str) -> RevealOutcome {
    let Ok(args) = serde_wasm_bindgen::to_value(&RevealArgs { name }) else {
        return RevealOutcome::Unreachable("the request could not be encoded".to_owned());
    };
    match invoke("secret_reveal", args).await {
        Ok(outcome) => serde_wasm_bindgen::from_value(outcome).unwrap_or_else(|_| {
            RevealOutcome::Unreachable("an unintelligible answer from the backend".to_owned())
        }),
        Err(error) => RevealOutcome::Unreachable(error_text(&error)),
    }
}

async fn save(name: &str, value: &str) -> Result<(), String> {
    let args = serde_wasm_bindgen::to_value(&SaveArgs { name, value })
        .map_err(|_| "the request could not be encoded".to_owned())?;
    invoke("secret_save", args)
        .await
        .map(|_| ())
        .map_err(|error| error_text(&error))
}

/// The window is the backend's to close, so this only asks.
fn close() {
    spawn_local(async {
        let _ = invoke("settings_close", JsValue::UNDEFINED).await;
    });
}

/// What Ok does, given what was loaded and what the box holds now.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum OkOutcome {
    /// Changed and non-empty: worth a keychain write.
    SaveThenClose,
    /// Unchanged, so no pointless keychain touch — on macOS a write can mean
    /// a permission prompt. Or emptied, because clearing a secret is
    /// deliberately not a feature of this slice. Esc always lands here.
    CloseUntouched,
}

pub(crate) fn ok_outcome(loaded: &str, current: &str) -> OkOutcome {
    if current.is_empty() || current == loaded {
        OkOutcome::CloseUntouched
    } else {
        OkOutcome::SaveThenClose
    }
}

/// The settings window's whole content. A fresh window every open, so the
/// eyeball starts hidden and the row starts from whatever the keystore says.
#[component]
pub fn SettingsPage() -> impl IntoView {
    // What came out of the keystore, for Ok to compare against.
    let loaded = RwSignal::new(String::new());
    // The box. The only home the bytes have on this side of IPC.
    let value = RwSignal::new(String::new());
    let revealed = RwSignal::new(false);
    let note = RwSignal::new(None::<String>);

    spawn_local(async move {
        match reveal(ANTHROPIC_NAME).await {
            RevealOutcome::Found(secret) => {
                loaded.set(secret.clone());
                value.set(secret);
            }
            RevealOutcome::Absent => {}
            RevealOutcome::Unreachable(reason) => {
                note.set(Some(format!(
                    "The system keychain couldn't be reached: {reason}"
                )));
            }
        }
    });

    // Esc: close, nothing changed, regardless of box contents.
    let handle = window_event_listener(ev::keydown, move |event| {
        if event.key() == "Escape" {
            close();
        }
    });
    on_cleanup(move || handle.remove());

    let ok = move |_| match ok_outcome(&loaded.get(), &value.get()) {
        OkOutcome::CloseUntouched => close(),
        OkOutcome::SaveThenClose => spawn_local(async move {
            match save(ANTHROPIC_NAME, &value.get_untracked()).await {
                Ok(()) => close(),
                Err(reason) => note.set(Some(reason)),
            }
        }),
    };

    view! {
        <main class="flex h-screen flex-col bg-neutral-50 p-6 dark:bg-neutral-900">
            // The list of secret rows. One today; more rows are more <li>s.
            <ul class="flex flex-col gap-3">
                <li class="flex items-center gap-3">
                    <label
                        for="secret-anthropic"
                        class="w-24 shrink-0 text-sm text-neutral-600 dark:text-neutral-400"
                    >
                        {ANTHROPIC_LABEL}
                    </label>
                    <input
                        id="secret-anthropic"
                        type=move || if revealed.get() { "text" } else { "password" }
                        autocomplete="off"
                        class="min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-3 py-1.5 font-mono text-sm text-neutral-900 focus:border-[#00b377] focus:outline-none dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-100 dark:focus:border-[#00e599]"
                        prop:value=value
                        on:input=move |ev| value.set(event_target_value(&ev))
                    />
                    <button
                        type="button"
                        aria-label="Reveal the secret"
                        class="shrink-0 rounded-md p-1.5 text-neutral-500 hover:bg-neutral-200 hover:text-neutral-700 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
                        on:click=move |_| revealed.update(|shown| *shown = !*shown)
                    >
                        <svg
                            class="h-4 w-4"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        >
                            <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7Z" />
                            <circle cx="12" cy="12" r="3" />
                            <Show when=move || revealed.get()>
                                <line x1="4" y1="20" x2="20" y2="4" />
                            </Show>
                        </svg>
                    </button>
                </li>
            </ul>
            <Show when=move || note.get().is_some()>
                <p class="mt-3 text-sm text-[#d4940a] dark:text-[#f5a623]">{move || note.get()}</p>
            </Show>
            <div class="mt-auto flex justify-end pt-4">
                <button
                    type="button"
                    class="rounded-md bg-[#00b377] px-4 py-1.5 text-sm font-medium text-white hover:bg-[#009966] dark:bg-[#00e599] dark:text-neutral-950 dark:hover:bg-[#33edb3]"
                    on:click=ok
                >
                    "Ok"
                </button>
            </div>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_secret_is_saved_on_ok() {
        assert_eq!(ok_outcome("sk-old", "sk-new"), OkOutcome::SaveThenClose);
    }

    #[test]
    fn a_first_secret_is_saved_on_ok() {
        assert_eq!(ok_outcome("", "sk-first"), OkOutcome::SaveThenClose);
    }

    #[test]
    fn an_unchanged_secret_closes_without_a_keychain_touch() {
        assert_eq!(ok_outcome("sk-kept", "sk-kept"), OkOutcome::CloseUntouched);
    }

    #[test]
    fn an_emptied_box_closes_without_clearing_anything() {
        assert_eq!(ok_outcome("sk-kept", ""), OkOutcome::CloseUntouched);
    }

    #[test]
    fn an_empty_box_over_no_secret_closes_without_writing() {
        assert_eq!(ok_outcome("", ""), OkOutcome::CloseUntouched);
    }
}
