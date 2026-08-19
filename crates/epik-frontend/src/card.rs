//! The one card in the codebase — and its one live relative.
//!
//! A card is how the transcript shows an observed act — a tool call, a
//! tool's answer, a turn that failed, a question settled — as distinct
//! from something a speaker said. One component renders all of them from
//! a [`Spec`]: a title, a body, and a tone. Cards carry the
//! interactivity in this window: a machine body that is a JSON document
//! delegates its own — per-node folding — to [`JsonTree`]; any other
//! body past its preview gets the click-to-expand toggle.
//!
//! A *pending* question is the family's one live member: the same
//! chrome, with the input the answer needs. The card owns the modality —
//! a path field, a native Browse dialog, a decline — and nothing else:
//! it sends the answer through [`ipc::answer_question`] and then waits,
//! disabled, for the `QuestionResolved` event to replace it. It never
//! updates itself; the window shows nothing it didn't receive.

use epik::chat::{Answer, Ask, TranscriptItem};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::ipc;
use crate::json::{self, JsonTree};

/// How a card carries itself: matter-of-fact, or bad news.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    Neutral,
    Error,
}

/// Everything a card is, decided before any rendering: the pure mapping
/// the tests pin down.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Spec {
    pub(crate) title: Option<String>,
    pub(crate) body: String,
    pub(crate) tone: Tone,
    /// Machine text — arguments, results — is monospace; prose is not.
    pub(crate) mono: bool,
}

/// A card title for `ask`: what kind of thing was asked for.
pub(crate) const fn ask_title(ask: &Ask) -> &'static str {
    match ask {
        Ask::Repository { .. } => "repository",
    }
}

/// The card for `item`, if `item` is card-shaped: tool calls, tool
/// results, failures, and settled questions are; speech is not, and
/// neither is a pending question, which is a [`QuestionCard`].
pub(crate) fn spec(item: &TranscriptItem) -> Option<Spec> {
    match item {
        TranscriptItem::QuestionResolved { ask, answer, .. } => Some(Spec {
            title: Some(ask_title(ask).to_owned()),
            body: match answer {
                Answer::Repository { url } => url.clone(),
                Answer::Declined => "declined".to_owned(),
            },
            tone: Tone::Neutral,
            mono: false,
        }),
        TranscriptItem::ToolCall { name, arguments } => Some(Spec {
            title: Some(name.clone()),
            body: arguments.clone(),
            tone: Tone::Neutral,
            mono: true,
        }),
        TranscriptItem::ToolResult { name, ok, content } => Some(Spec {
            title: Some(name.clone()),
            body: content.clone(),
            tone: if *ok { Tone::Neutral } else { Tone::Error },
            mono: true,
        }),
        TranscriptItem::TurnFailed { reason } => Some(Spec {
            title: None,
            body: reason.clone(),
            tone: Tone::Error,
            mono: false,
        }),
        TranscriptItem::Message { .. }
        | TranscriptItem::AssistantDelta { .. }
        | TranscriptItem::Question { .. } => None,
    }
}

/// How many characters of body a folded card shows.
pub(crate) const PREVIEW_CAP: usize = 200;

/// The state logic of the expand toggle, as a pure function: what the
/// card displays for `body` at `expanded`, and whether there is a toggle
/// at all. A body within [`PREVIEW_CAP`] has nothing to expand.
pub(crate) fn shown(body: &str, expanded: bool) -> (String, bool) {
    if body.chars().count() <= PREVIEW_CAP {
        return (body.to_owned(), false);
    }
    if expanded {
        return (body.to_owned(), true);
    }
    let mut preview: String = body.chars().take(PREVIEW_CAP).collect();
    preview.push('…');
    (preview, true)
}

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
pub(crate) fn Card(spec: Spec) -> impl IntoView {
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

/// A pending question, in the card family: the persona's prompt, and
/// the affordances its answer needs. Every control disables on submit;
/// the card then waits for the resolution to come back as an event.
#[component]
pub(crate) fn QuestionCard(id: String, ask: Ask) -> impl IntoView {
    let title = ask_title(&ask);
    let Ask::Repository { prompt } = ask;
    let path = RwSignal::new(String::new());
    let submitted = RwSignal::new(false);
    let note = RwSignal::new(None::<String>);
    let id = StoredValue::new(id);

    // One door for both answers: claim the card, send, and stay disabled
    // — the reply, or the refusal, is the backend's to give.
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
            <div class=TITLE>{title}</div>
            <div class="break-words whitespace-pre-wrap">{prompt}</div>
            <div class="mt-2 flex items-center gap-2">
                <input
                    type="text"
                    placeholder="/path/to/repository.git"
                    autocomplete="off"
                    spellcheck="false"
                    class="min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-2.5 py-1 font-mono text-xs text-neutral-900 focus:border-[#00b377] focus:outline-none disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-100 dark:focus:border-[#00e599]"
                    prop:value=path
                    prop:disabled=submitted
                    on:input=move |event| path.set(event_target_value(&event))
                    on:keydown=move |event: ev::KeyboardEvent| {
                        if event.key() == "Enter" {
                            event.prevent_default();
                            confirm();
                        }
                    }
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
            <Show when=move || note.get().is_some()>
                <p class="mt-1 text-xs text-[#d4940a] dark:text-[#f5a623]">{move || note.get()}</p>
            </Show>
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settled_question_is_a_neutral_prose_card_titled_by_its_kind() {
        let ask = Ask::Repository {
            prompt: "Where should this live?".to_owned(),
        };
        let answered = TranscriptItem::QuestionResolved {
            id: "1".to_owned(),
            ask: ask.clone(),
            answer: Answer::Repository {
                url: "/tmp/wumpus.git".to_owned(),
            },
        };
        assert_eq!(
            spec(&answered),
            Some(Spec {
                title: Some("repository".to_owned()),
                body: "/tmp/wumpus.git".to_owned(),
                tone: Tone::Neutral,
                mono: false,
            })
        );

        let declined = TranscriptItem::QuestionResolved {
            id: "2".to_owned(),
            ask,
            answer: Answer::Declined,
        };
        assert_eq!(
            spec(&declined),
            Some(Spec {
                title: Some("repository".to_owned()),
                body: "declined".to_owned(),
                tone: Tone::Neutral,
                mono: false,
            })
        );
    }

    #[test]
    fn a_pending_question_is_not_a_static_card() {
        assert_eq!(
            spec(&TranscriptItem::Question {
                id: "1".to_owned(),
                ask: Ask::Repository {
                    prompt: "Where?".to_owned(),
                },
            }),
            None
        );
    }

    #[test]
    fn a_tool_call_is_a_neutral_mono_card_titled_by_its_tool() {
        let item = TranscriptItem::ToolCall {
            name: "current_time".to_owned(),
            arguments: "{}".to_owned(),
        };
        assert_eq!(
            spec(&item),
            Some(Spec {
                title: Some("current_time".to_owned()),
                body: "{}".to_owned(),
                tone: Tone::Neutral,
                mono: true,
            })
        );
    }

    #[test]
    fn a_tool_result_wears_its_outcome_as_tone() {
        let ok = TranscriptItem::ToolResult {
            name: "current_time".to_owned(),
            ok: true,
            content: "{}".to_owned(),
        };
        assert_eq!(spec(&ok).unwrap().tone, Tone::Neutral);

        let failed = TranscriptItem::ToolResult {
            name: "current_time".to_owned(),
            ok: false,
            content: "no such day".to_owned(),
        };
        assert_eq!(spec(&failed).unwrap().tone, Tone::Error);
    }

    #[test]
    fn a_failed_turn_is_an_untitled_error_card_in_prose() {
        let item = TranscriptItem::TurnFailed {
            reason: "the wire went quiet".to_owned(),
        };
        assert_eq!(
            spec(&item),
            Some(Spec {
                title: None,
                body: "the wire went quiet".to_owned(),
                tone: Tone::Error,
                mono: false,
            })
        );
    }

    #[test]
    fn speech_is_never_a_card() {
        assert_eq!(
            spec(&TranscriptItem::Message {
                role: epik::chat::Role::User,
                text: "hi".to_owned(),
            }),
            None
        );
        assert_eq!(
            spec(&TranscriptItem::AssistantDelta {
                text: "he".to_owned(),
            }),
            None
        );
    }

    #[test]
    fn a_body_at_the_cap_has_nothing_to_expand() {
        let body = "x".repeat(PREVIEW_CAP);
        assert_eq!(shown(&body, false), (body.clone(), false));
        assert_eq!(shown(&body, true), (body, false));
    }

    #[test]
    fn one_past_the_cap_folds_with_an_ellipsis_and_a_toggle() {
        let body = "x".repeat(PREVIEW_CAP + 1);
        assert_eq!(
            shown(&body, false),
            (format!("{}…", "x".repeat(PREVIEW_CAP)), true)
        );
        assert_eq!(shown(&body, true), (body, true), "expanded shows it all");
    }
}
