//! The chat surface: the app's main view.
//!
//! The transcript the user sees is a pure fold over [`TranscriptItem`]s —
//! history from `get_transcript` first, then live events on top. Nothing
//! is shown that didn't come back over the barrier as an item. Message
//! text renders through a deterministic parse into escaped text, backtick
//! code spans, and http(s) autolinks — a chat message can never inject
//! markup.

use epik::chat::{Role, TranscriptItem};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::ipc;

/// Folds one arriving item into the transcript. A single arm today; new
/// item kinds become new arms here, never a second channel.
pub(crate) fn fold(transcript: &mut Vec<TranscriptItem>, item: TranscriptItem) {
    match item {
        TranscriptItem::Message { .. } => transcript.push(item),
    }
}

/// One run of a message, after parsing. What the renderer maps 1:1 into
/// the view, and what the golden tests pin down.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Segment {
    /// Escaped plain text, newlines preserved.
    Text(String),
    /// A `backtick span`, rendered monospace.
    Code(String),
    /// A bare http(s) URL, rendered as a link that opens the system browser.
    Link(String),
}

/// Punctuation that ends a sentence rather than a URL.
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')'];

/// Splits `text` into text runs and bare http(s) links.
fn push_text_and_links(segments: &mut Vec<Segment>, text: &str) {
    let mut rest = text;
    loop {
        let start = ["http://", "https://"]
            .iter()
            .filter_map(|scheme| rest.find(scheme))
            .min();
        let Some(start) = start else { break };
        let end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |length| start + length);
        let url = rest[start..end].trim_end_matches(TRAILING);
        if !rest[..start].is_empty() {
            segments.push(Segment::Text(rest[..start].to_owned()));
        }
        segments.push(Segment::Link(url.to_owned()));
        rest = &rest[start + url.len()..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Text(rest.to_owned()));
    }
}

/// The deterministic heart of message rendering: text in, structure out.
///
/// Backtick pairs become code spans (an unmatched backtick is just text),
/// bare http(s) URLs in the remaining text become links with sentence
/// punctuation left outside, and everything else stays text. That is the
/// whole formatting language of this slice.
pub(crate) fn parse(text: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let Some(length) = rest[open + 1..].find('`') else {
            break;
        };
        push_text_and_links(&mut segments, &rest[..open]);
        segments.push(Segment::Code(rest[open + 1..open + 1 + length].to_owned()));
        rest = &rest[open + length + 2..];
    }
    push_text_and_links(&mut segments, rest);
    segments
}

/// Maps a message's parse 1:1 into the view. Text lands in text nodes —
/// escaped by construction — so markup in a message stays words.
fn render_message(text: &str) -> impl IntoView + use<> {
    parse(text)
        .into_iter()
        .map(|segment| match segment {
            Segment::Text(text) => text.into_any(),
            Segment::Code(code) => view! {
                <code class="rounded bg-black/10 px-1 font-mono text-[0.9em] dark:bg-white/10">
                    {code}
                </code>
            }
            .into_any(),
            Segment::Link(url) => {
                let href = url.clone();
                let target = url.clone();
                view! {
                    <a
                        href=href
                        class="cursor-pointer underline decoration-current/50 underline-offset-2 hover:decoration-current"
                        on:click=move |event| {
                            event.prevent_default();
                            ipc::open_url(target.clone());
                        }
                    >
                        {url}
                    </a>
                }
                .into_any()
            }
        })
        .collect_view()
}

/// One bubble: the user against the right edge in the accent, the
/// assistant against the left in neutral — styled and folded evenly even
/// though nothing produces assistant items yet.
fn bubble(item: &TranscriptItem) -> impl IntoView + use<> {
    let TranscriptItem::Message { role, text } = item;
    let side = match role {
        Role::User => "self-end bg-[#00b377] text-white dark:bg-[#00e599] dark:text-neutral-950",
        Role::Assistant => {
            "self-start border border-neutral-200 bg-white text-neutral-900 \
             dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
        }
    };
    view! {
        <li class=format!(
            "max-w-[85%] rounded-2xl px-3.5 py-2 text-sm leading-relaxed \
             break-words whitespace-pre-wrap {side}",
        )>{render_message(text)}</li>
    }
}

/// The chat surface: transcript above, prose input below.
#[component]
pub fn Chat() -> impl IntoView {
    let transcript = RwSignal::new(Vec::<TranscriptItem>::new());
    let draft = RwSignal::new(String::new());
    // Whether the view is pinned to the newest message. Scrolling up to
    // read history unpins; returning to the bottom pins again.
    let pinned = RwSignal::new(true);
    let pane = NodeRef::<leptos::html::Div>::new();
    let input = NodeRef::<leptos::html::Textarea>::new();

    // History first, then live items, through the same fold.
    spawn_local(async move {
        for item in ipc::get_transcript().await {
            transcript.update(|transcript| fold(transcript, item));
        }
    });
    ipc::listen_transcript(move |item| {
        transcript.update(|transcript| fold(transcript, item));
    });

    // Follow the newest message, unless the user has wandered up.
    Effect::new(move |_| {
        transcript.track();
        if pinned.get_untracked() {
            request_animation_frame(move || {
                if let Some(pane) = pane.get_untracked() {
                    pane.set_scroll_top(pane.scroll_height());
                }
            });
        }
    });

    // Whether the app is currently dark. The media query is the source of
    // truth: it follows the system until the first click pins a theme, and
    // it keeps talking if the OS switches under a still-unpinned app — so
    // the icon mirrors it, initially and on every change. It drives only
    // the icon; the styling follows the media query on its own.
    let dark = RwSignal::new(false);
    if let Ok(Some(query)) = window().match_media("(prefers-color-scheme: dark)") {
        dark.set(query.matches());
        let mirror = Closure::<dyn FnMut()>::new(move || {
            if let Ok(Some(query)) = window().match_media("(prefers-color-scheme: dark)") {
                dark.set(query.matches());
            }
        });
        let _ = query.add_event_listener_with_callback("change", mirror.as_ref().unchecked_ref());
        mirror.forget();
    }
    let flip_theme = move |_| {
        let to_dark = !dark.get_untracked();
        ipc::set_theme(if to_dark { "dark" } else { "light" });
        dark.set(to_dark);
    };

    // `autocorrect` has no typed attribute in leptos; set it on the node.
    Effect::new(move |_| {
        if let Some(input) = input.get() {
            let _ = input.set_attribute("autocorrect", "on");
        }
    });

    let send = move || {
        let text = draft.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        ipc::send_message(text);
        draft.set(String::new());
        if let Some(input) = input.get_untracked() {
            let _ = input.focus();
        }
    };

    view! {
        <main class="relative flex h-screen flex-col bg-neutral-50 dark:bg-neutral-900">
            <button
                type="button"
                aria-label="Switch between light and dark"
                class="absolute top-3 right-3 z-10 rounded-md p-1.5 text-neutral-500 hover:bg-neutral-200 hover:text-neutral-700 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
                on:click=flip_theme
            >
                <svg
                    class="h-5 w-5"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.8"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                >
                    <Show
                        when=move || dark.get()
                        fallback=|| {
                            view! {
                                // The moon, shown in light mode: the way to the dark.
                                <path d="M21.752 15.002A9.72 9.72 0 0 1 18 15.75c-5.385 0-9.75-4.365-9.75-9.75 0-1.33.266-2.597.748-3.752A9.753 9.753 0 0 0 3 11.25C3 16.635 7.365 21 12.75 21a9.753 9.753 0 0 0 9.002-5.998Z" />
                            }
                        }
                    >
                        // The sun, shown in dark mode: the way to the light.
                        <path d="M12 3v2.25m6.364.386-1.591 1.591M21 12h-2.25m-.386 6.364-1.591-1.591M12 18.75V21m-4.773-4.227-1.591 1.591M5.25 12H3m4.227-4.773L5.636 5.636M15.75 12a3.75 3.75 0 1 1-7.5 0 3.75 3.75 0 0 1 7.5 0Z" />
                    </Show>
                </svg>
            </button>
            <div
                node_ref=pane
                on:scroll=move |_| {
                    if let Some(pane) = pane.get_untracked() {
                        let bottom = pane.scroll_top() + pane.client_height();
                        pinned.set(bottom >= pane.scroll_height() - 48);
                    }
                }
                class="min-h-0 flex-1 overflow-y-auto px-4 py-6 [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-neutral-300 dark:[&::-webkit-scrollbar-thumb]:bg-neutral-700"
            >
                <ul class="mx-auto flex max-w-2xl flex-col gap-3">
                    <For
                        each=move || transcript.get().into_iter().enumerate()
                        key=|(index, _)| *index
                        children=|(_, item)| bubble(&item)
                    />
                </ul>
            </div>
            <div class="shrink-0 border-t border-neutral-200 p-3 dark:border-neutral-800">
                <div class="mx-auto flex max-w-2xl items-end gap-2">
                    <textarea
                        node_ref=input
                        rows=2
                        placeholder="Message"
                        spellcheck="true"
                        autocapitalize="sentences"
                        class="max-h-[60vh] min-h-10 flex-1 resize-y rounded-lg border border-neutral-300 bg-white px-3 py-2 text-sm text-neutral-900 focus:border-[#00b377] focus:outline-none dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-100 dark:focus:border-[#00e599]"
                        prop:value=draft
                        on:input=move |event| draft.set(event_target_value(&event))
                        on:keydown=move |event: ev::KeyboardEvent| {
                            if event.key() == "Enter" && !event.shift_key() {
                                event.prevent_default();
                                send();
                            }
                        }
                    ></textarea>
                    <button
                        type="button"
                        aria-label="Send"
                        class="shrink-0 rounded-lg bg-[#00b377] p-2 text-white hover:bg-[#009966] dark:bg-[#00e599] dark:text-neutral-950 dark:hover:bg-[#33edb3]"
                        on:click=move |_| send()
                    >
                        <svg
                            class="h-5 w-5"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="1.8"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        >
                            <path d="M6 12 3.269 3.125A59.769 59.769 0 0 1 21.485 12 59.768 59.768 0 0 1 3.27 20.875L5.999 12Zm0 0h7.5" />
                        </svg>
                    </button>
                </div>
            </div>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: Role, text: &str) -> TranscriptItem {
        TranscriptItem::Message {
            role,
            text: text.to_owned(),
        }
    }

    fn text(s: &str) -> Segment {
        Segment::Text(s.to_owned())
    }

    fn code(s: &str) -> Segment {
        Segment::Code(s.to_owned())
    }

    fn link(s: &str) -> Segment {
        Segment::Link(s.to_owned())
    }

    #[test]
    fn the_fold_appends_items_in_order() {
        let mut transcript = Vec::new();
        fold(&mut transcript, message(Role::User, "first"));
        fold(&mut transcript, message(Role::User, "second"));
        assert_eq!(
            transcript,
            [message(Role::User, "first"), message(Role::User, "second")]
        );
    }

    #[test]
    fn the_fold_takes_the_assistant_arm_even_before_anything_answers() {
        let mut transcript = Vec::new();
        fold(&mut transcript, message(Role::Assistant, "hello yourself"));
        assert_eq!(transcript, [message(Role::Assistant, "hello yourself")]);
    }

    #[test]
    fn plain_text_stays_one_text_run() {
        assert_eq!(parse("just words"), [text("just words")]);
    }

    #[test]
    fn newlines_survive_parsing() {
        assert_eq!(parse("line one\nline two"), [text("line one\nline two")]);
    }

    #[test]
    fn an_attempted_script_tag_arrives_as_text() {
        assert_eq!(
            parse("<script>alert(1)</script>"),
            [text("<script>alert(1)</script>")]
        );
    }

    #[test]
    fn a_backtick_span_becomes_code() {
        assert_eq!(
            parse("run `cargo test` now"),
            [text("run "), code("cargo test"), text(" now")]
        );
    }

    #[test]
    fn backticks_at_the_edges_still_pair() {
        assert_eq!(
            parse("`start` and `end`"),
            [code("start"), text(" and "), code("end"),]
        );
    }

    #[test]
    fn an_unmatched_backtick_is_just_text() {
        assert_eq!(parse("a ` b"), [text("a ` b")]);
    }

    #[test]
    fn a_bare_url_becomes_a_link() {
        assert_eq!(
            parse("see https://example.com for more"),
            [text("see "), link("https://example.com"), text(" for more"),]
        );
    }

    #[test]
    fn http_links_too() {
        assert_eq!(parse("http://example.com"), [link("http://example.com")]);
    }

    #[test]
    fn sentence_punctuation_stays_outside_the_link() {
        assert_eq!(
            parse("read https://example.com/docs."),
            [text("read "), link("https://example.com/docs"), text(".")]
        );
    }

    #[test]
    fn a_parenthesized_url_keeps_its_parenthesis_outside() {
        assert_eq!(
            parse("(https://example.com)"),
            [text("("), link("https://example.com"), text(")")]
        );
    }

    #[test]
    fn nothing_else_becomes_a_link() {
        assert_eq!(
            parse("ftp://example.com and www.example.com"),
            [text("ftp://example.com and www.example.com")]
        );
    }

    #[test]
    fn a_url_inside_backticks_is_code_not_a_link() {
        assert_eq!(
            parse("`https://example.com`"),
            [code("https://example.com")]
        );
    }
}
