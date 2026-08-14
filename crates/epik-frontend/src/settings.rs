//! The settings window: a list of labeled secret rows — Anthropic and
//! GitHub, with identical semantics.
//!
//! A secret's bytes live in exactly one place on this side: the password
//! input's reactive state. They arrive there through [`ipc::reveal_secret`]
//! and leave through [`ipc::save_secret`], and nowhere else — never a log
//! line, never a persisted structure.

use epik::keystore::{Resolved, Secret};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::ipc;

/// The rows, in display order: keystore name and label.
const ROWS: [(&str, &str); 2] = [
    ("ANTHROPIC_API_KEY", "Anthropic"),
    ("GITHUB_TOKEN", "GitHub"),
];

/// What Ok does to one row, given what was loaded and what the box holds
/// now.
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

/// One row's reactive state. `RwSignal` is `Copy`, so the whole struct is.
#[derive(Clone, Copy)]
struct Row {
    /// What came out of the keystore, for Ok to compare against.
    loaded: RwSignal<String>,
    /// The box. The only home the bytes have on this side of IPC.
    value: RwSignal<String>,
    revealed: RwSignal<bool>,
}

impl Row {
    fn new() -> Self {
        Self {
            loaded: RwSignal::new(String::new()),
            value: RwSignal::new(String::new()),
            revealed: RwSignal::new(false),
        }
    }
}

/// The settings window's whole content. A fresh window every open, so the
/// eyeballs start hidden and each row starts from whatever the keystore
/// says.
#[component]
pub fn SettingsPage() -> impl IntoView {
    let rows: Vec<(&'static str, &'static str, Row)> = ROWS
        .iter()
        .map(|&(name, label)| (name, label, Row::new()))
        .collect();
    let note = RwSignal::new(None::<String>);

    for &(name, _, row) in &rows {
        spawn_local(async move {
            match ipc::reveal_secret(name).await {
                Resolved::Found(secret) => {
                    row.loaded.set(secret.reveal().to_owned());
                    row.value.set(secret.reveal().to_owned());
                }
                Resolved::Absent => {}
                Resolved::Unreachable(reason) => {
                    note.set(Some(format!(
                        "The system keychain couldn't be reached: {reason}"
                    )));
                }
            }
        });
    }

    // Esc: close, nothing changed, regardless of box contents.
    let handle = window_event_listener(ev::keydown, move |event| {
        if event.key() == "Escape" {
            ipc::close_settings();
        }
    });
    on_cleanup(move || handle.remove());

    let saves: Vec<(&'static str, Row)> = rows.iter().map(|&(name, _, row)| (name, row)).collect();
    let ok = move |_| {
        let due: Vec<(&'static str, String)> = saves
            .iter()
            .filter(|(_, row)| {
                ok_outcome(&row.loaded.get_untracked(), &row.value.get_untracked())
                    == OkOutcome::SaveThenClose
            })
            .map(|&(name, row)| (name, row.value.get_untracked()))
            .collect();
        spawn_local(async move {
            for (name, value) in due {
                if let Err(reason) = ipc::save_secret(name, &Secret::from(value)).await {
                    note.set(Some(reason));
                    return;
                }
            }
            ipc::close_settings();
        });
    };

    view! {
        <main class="flex h-screen flex-col bg-neutral-50 p-6 dark:bg-neutral-900">
            // The list of secret rows; more rows are more entries in ROWS.
            <ul class="flex flex-col gap-3">
                {rows
                    .iter()
                    .map(|&(name, label, row)| secret_row(name, label, row))
                    .collect_view()}
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

/// One labeled secret row: the password box and its eyeball.
fn secret_row(name: &'static str, label: &'static str, row: Row) -> impl IntoView {
    let id = format!("secret-{}", name.to_lowercase());
    view! {
        <li class="flex items-center gap-3">
            <label
                for=id.clone()
                class="w-24 shrink-0 text-sm text-neutral-600 dark:text-neutral-400"
            >
                {label}
            </label>
            <input
                id=id
                type=move || if row.revealed.get() { "text" } else { "password" }
                autocomplete="off"
                class="min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-3 py-1.5 font-mono text-sm text-neutral-900 focus:border-[#00b377] focus:outline-none dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-100 dark:focus:border-[#00e599]"
                prop:value=row.value
                on:input=move |ev| row.value.set(event_target_value(&ev))
            />
            <button
                type="button"
                aria-label="Reveal the secret"
                class="shrink-0 rounded-md p-1.5 text-neutral-500 hover:bg-neutral-200 hover:text-neutral-700 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
                on:click=move |_| row.revealed.update(|shown| *shown = !*shown)
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
                    <Show when=move || row.revealed.get()>
                        <line x1="4" y1="20" x2="20" y2="4" />
                    </Show>
                </svg>
            </button>
        </li>
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

    #[test]
    fn the_rows_are_anthropic_then_github() {
        assert_eq!(
            ROWS,
            [
                ("ANTHROPIC_API_KEY", "Anthropic"),
                ("GITHUB_TOKEN", "GitHub")
            ]
        );
    }
}
