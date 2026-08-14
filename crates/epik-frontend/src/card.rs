//! The one card in the codebase.
//!
//! A card is how the transcript shows an observed act — a tool call, a
//! tool's answer, a turn that failed — as distinct from something a
//! speaker said. One component renders all of them from a [`Spec`]: a
//! title, a body, a tone, and — when the body outgrows its preview — a
//! click-to-expand toggle, the component's only interactivity.

use epik::chat::TranscriptItem;
use leptos::prelude::*;

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

/// The card for `item`, if `item` is card-shaped: tool calls, tool
/// results, and failures are; speech is not.
pub(crate) fn spec(item: &TranscriptItem) -> Option<Spec> {
    match item {
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
        TranscriptItem::Message { .. } | TranscriptItem::AssistantDelta { .. } => None,
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

/// One card in the transcript flow: full-width where bubbles hug their
/// side, bordered where bubbles are filled — an observation, not speech.
#[component]
pub(crate) fn Card(spec: Spec) -> impl IntoView {
    let expanded = RwSignal::new(false);
    let tone = match spec.tone {
        Tone::Neutral => {
            "border-neutral-200 bg-neutral-100 text-neutral-600 \
             dark:border-neutral-700 dark:bg-neutral-800/60 dark:text-neutral-300"
        }
        Tone::Error => {
            "border-[#dc2626]/30 bg-[#dc2626]/5 text-[#dc2626] \
             dark:border-[#ef4444]/30 dark:bg-[#ef4444]/10 dark:text-[#ef4444]"
        }
    };
    let body_class = if spec.mono {
        "font-mono text-xs break-words whitespace-pre-wrap"
    } else {
        "break-words whitespace-pre-wrap"
    };
    let body = spec.body;
    view! {
        <li class=format!("self-stretch rounded-lg border px-3.5 py-2 text-sm {tone}")>
            {spec
                .title
                .map(|title| {
                    view! {
                        <div class="mb-1 font-mono text-xs font-semibold tracking-wide opacity-70">
                            {title}
                        </div>
                    }
                })}
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
