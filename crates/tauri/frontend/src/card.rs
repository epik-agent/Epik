//! The one card in the codebase — and its live relatives.
//!
//! A card is how the transcript shows an observed act — a tool call, a
//! tool's answer, a turn that failed, a question settled — as distinct
//! from something a speaker said. One component renders all of them from
//! an [`assistant::Card`]: a title, a body, and a tone. Cards carry the
//! interactivity in this window: a machine body that is a JSON document
//! delegates its own — per-node folding — to [`JsonTree`]; any other
//! body past its preview gets the click-to-expand toggle.
//!
//! A *pending* question is the family's live side: the same chrome,
//! with the input the answer needs — one card per ask kind, because the
//! card owns the whole modality. The repository card has a path field,
//! a native Browse dialog, and a decline; the check card has a command
//! field prefilled from detection, a confirm, and the skip. Each sends
//! its answer through [`ipc::answer_question`] and then waits, disabled,
//! for the `QuestionResolved` event to replace it. It never updates
//! itself; the window shows nothing it didn't receive.

use epik_core::assistant::{self, Answer, Ask, Tone, shown};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::ipc;
use crate::json::{self, JsonTree};

/// What every card wears; the tone colors it.
const CHROME: &str = "self-stretch rounded-lg border px-3.5 py-2 text-sm";
/// What a card's title wears.
const TITLE: &str = "mb-1 font-mono text-xs font-semibold tracking-wide opacity-70";

const fn tone_class(tone: Tone) -> &'static str {
    match tone {
        Tone::Neutral => {
            "border-neutral-200 bg-neutral-100 text-neutral-600 \
             dark:border-neutral-700 dark:bg-neutral-800/60 dark:text-neutral-300"
        }
        Tone::Error => {
            "border-[#dc2626]/30 bg-[#dc2626]/5 text-[#dc2626] \
             dark:border-[#ef4444]/30 dark:bg-[#ef4444]/10 dark:text-[#ef4444]"
        }
    }
}

/// One card in the transcript flow: full-width where bubbles hug their
/// side, bordered where bubbles are filled — an observation, not speech.
/// A mono body that is a JSON document renders as a folding
/// [`JsonTree`]; every other body is the folded-or-whole text.
#[component]
pub fn Card(spec: assistant::Card) -> impl IntoView {
    let tone = tone_class(spec.tone);
    let title = spec
        .title
        .map(|title| view! { <div class=TITLE>{title}</div> });
    let body = spec.body;
    if spec.mono && json::rows(&body, &std::collections::HashSet::new()).is_some() {
        return view! {
            <li class=format!("{CHROME} {tone}")>
                {title}
                <div class="font-mono text-xs">
                    <JsonTree text=body />
                </div>
            </li>
        }
        .into_any();
    }
    let expanded = RwSignal::new(false);
    let body_class = if spec.mono {
        "font-mono text-xs break-words whitespace-pre-wrap"
    } else {
        "break-words whitespace-pre-wrap"
    };
    view! {
        <li class=format!("{CHROME} {tone}")>
            {title}
            <div class=body_class>{
                let body = body.clone();
                move || shown(&body, expanded.get()).0
            }</div>
            <Show when={
                let body = body.clone();
                move || shown(&body, expanded.get()).1
            }>
                <button
                    type="button"
                    class="mt-1 cursor-pointer text-xs underline opacity-70 hover:opacity-100"
                    on:click=move |_| expanded.update(|expanded| *expanded = !*expanded)
                >
                    {move || if expanded.get() { "show less" } else { "show all" }}
                </button>
            </Show>
        </li>
    }
    .into_any()
}

/// What a card's small buttons wear.
const BUTTON: &str = "shrink-0 rounded-md border border-neutral-300 px-2.5 py-1 text-xs \
                      hover:bg-neutral-200 disabled:cursor-not-allowed disabled:opacity-40 \
                      disabled:hover:bg-transparent dark:border-neutral-600 \
                      dark:hover:bg-neutral-700";
/// What the confirming button wears: the accent, like Send.
const CONFIRM: &str = "shrink-0 rounded-md bg-[#00b377] px-2.5 py-1 text-xs font-medium \
                       text-white hover:bg-[#009966] disabled:cursor-not-allowed \
                       disabled:opacity-40 disabled:hover:bg-[#00b377] dark:bg-[#00e599] \
                       dark:text-neutral-950 dark:hover:bg-[#33edb3] \
                       dark:disabled:hover:bg-[#00e599]";

/// What a question card's text input wears.
const FIELD: &str = "min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-2.5 py-1 \
                     font-mono text-xs text-neutral-900 focus:border-[#00b377] \
                     focus:outline-none disabled:opacity-60 dark:border-neutral-700 \
                     dark:bg-neutral-950 dark:text-neutral-100 dark:focus:border-[#00e599]";

/// A pending question, in the card family: the persona's prompt, and
/// the affordances its answer needs. Every control disables on submit;
/// the card then waits for the resolution to come back as an event.
/// Each ask kind is its own live card — the backend says what it wants,
/// and the modality is entirely here.
#[component]
pub fn QuestionCard(id: String, ask: Ask) -> impl IntoView {
    match ask {
        Ask::Repository { prompt } => view! { <RepositoryQuestion id prompt /> }.into_any(),
        Ask::Check { prompt, proposal } => {
            view! { <CheckQuestion id prompt proposal /> }.into_any()
        }
    }
}

/// A pending question's standing — whether it has been answered, and
/// the refusal if the backend gave one — and the one door for its
/// answers: claims the card, sends, and stays disabled, since the reply
/// or the refusal is the backend's to give.
fn pending(
    id: String,
) -> (
    RwSignal<bool>,
    RwSignal<Option<String>>,
    impl Fn(Answer) + Copy,
) {
    let id = StoredValue::new(id);
    let submitted = RwSignal::new(false);
    let note = RwSignal::new(None::<String>);
    let answer_with = move |answer: Answer| {
        if submitted.get_untracked() {
            return;
        }
        submitted.set(true);
        spawn_local(async move {
            if let Err(reason) = ipc::answer_question(&id.get_value(), &answer).await {
                note.set(Some(reason));
            }
        });
    };
    (submitted, note, answer_with)
}

/// Enter in a question's field is its confirm.
fn on_enter(confirm: impl Fn() + Copy + 'static) -> impl Fn(ev::KeyboardEvent) {
    move |event| {
        if event.key() == "Enter" {
            event.prevent_default();
            confirm();
        }
    }
}

/// The backend's refusal, under the controls, once there is one.
#[component]
fn Note(note: RwSignal<Option<String>>) -> impl IntoView {
    view! {
        <Show when=move || note.get().is_some()>
            <p class="mt-1 text-xs text-[#d4940a] dark:text-[#f5a623]">{move || note.get()}</p>
        </Show>
    }
}

/// Where a repository should live: a path field, a native Browse
/// dialog, and a decline.
#[component]
fn RepositoryQuestion(id: String, prompt: String) -> impl IntoView {
    let path = RwSignal::new(String::new());
    let (submitted, note, answer_with) = pending(id);
    let confirm = move || {
        let url = path.get_untracked().trim().to_owned();
        if !url.is_empty() {
            answer_with(Answer::Repository { url });
        }
    };
    let browse = move |_| {
        spawn_local(async move {
            if let Some(chosen) = ipc::browse_repository().await {
                path.set(chosen);
            }
        });
    };

    view! {
        <li class=format!("{CHROME} {}", tone_class(Tone::Neutral))>
            <div class=TITLE>"repository"</div>
            <div class="break-words whitespace-pre-wrap">{prompt}</div>
            <div class="mt-2 flex items-center gap-2">
                <input
                    type="text"
                    placeholder="/path/to/repository.git"
                    autocomplete="off"
                    spellcheck="false"
                    class=FIELD
                    prop:value=path
                    prop:disabled=submitted
                    on:input=move |event| path.set(event_target_value(&event))
                    on:keydown=on_enter(confirm)
                />
                <button type="button" class=BUTTON prop:disabled=submitted on:click=browse>
                    "Browse…"
                </button>
                <button
                    type="button"
                    class=CONFIRM
                    prop:disabled=move || submitted.get() || path.get().trim().is_empty()
                    on:click=move |_| confirm()
                >
                    "Use this"
                </button>
                <button
                    type="button"
                    class=BUTTON
                    prop:disabled=submitted
                    on:click=move |_| answer_with(Answer::Declined)
                >
                    "Never mind"
                </button>
            </div>
            <Note note />
        </li>
    }
}

/// The check a feature build will be judged by: a command field
/// prefilled from detection — a proposal, not a decision — a confirm,
/// and the skip. Skipping is a first-class answer, at the user's stated
/// risk: the build then runs on observation alone.
#[component]
fn CheckQuestion(id: String, prompt: String, proposal: Option<String>) -> impl IntoView {
    let command = RwSignal::new(proposal.unwrap_or_default());
    let (submitted, note, answer_with) = pending(id);
    let confirm = move || {
        let command = command.get_untracked().trim().to_owned();
        if !command.is_empty() {
            answer_with(Answer::Check { command });
        }
    };

    view! {
        <li class=format!("{CHROME} {}", tone_class(Tone::Neutral))>
            <div class=TITLE>"check"</div>
            <div class="break-words whitespace-pre-wrap">{prompt}</div>
            <div class="mt-2 flex items-center gap-2">
                <input
                    type="text"
                    placeholder="the command that says green"
                    autocomplete="off"
                    spellcheck="false"
                    class=FIELD
                    prop:value=command
                    prop:disabled=submitted
                    on:input=move |event| command.set(event_target_value(&event))
                    on:keydown=on_enter(confirm)
                />
                <button
                    type="button"
                    class=CONFIRM
                    prop:disabled=move || submitted.get() || command.get().trim().is_empty()
                    on:click=move |_| confirm()
                >
                    "Run this check"
                </button>
                <button
                    type="button"
                    class=BUTTON
                    prop:disabled=submitted
                    on:click=move |_| answer_with(Answer::Declined)
                >
                    "Skip the check"
                </button>
            </div>
            <Note note />
        </li>
    }
}
