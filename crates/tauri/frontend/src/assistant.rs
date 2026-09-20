//! The assistant tab: the window's conversation with the assistant.
//!
//! `leptos_rich_chat`'s [`Chat`] draws it — the transcript that follows
//! its tail, the Markdown, math and highlighted code, the composer and
//! its preview — and [`epik_core::assistant`] decides what is in it. The
//! transcript the user sees is [`Transcript::fold`] over
//! [`TranscriptItem`]s — history from `get_transcript` first, then live
//! events on top — and each fold's [`Change`] is applied to the
//! [`Message`]s `Chat` shows. Speech is a bubble of Markdown; everything
//! that wasn't said — tool calls, tool results, failures, questions — is
//! a [`Card`] or a [`QuestionCard`] drawn as a [`leptos_rich_chat::Body::View`].

use epik_core::assistant::{
    Body, Change, Entry, Role, Segment, Transcript, TranscriptItem, segments,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::{
    Chat, Look, Message, Names, Position, RenderOptions, RichChatStyle, RichText,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::card::{Card, QuestionCard};
use crate::ipc;
use crate::json::JsonTree;

/// Whose messages sit where, in what colors: the user against the right
/// edge in the accent, the assistant against the left in the neutral
/// bubble — styles.css draws its border — and a card on the bare
/// transcript, which its root spans.
fn names() -> Names {
    Names::none()
        .name(
            "user",
            Look::at(Position::Right)
                .background("light-dark(#00b377, #00e599)")
                .foreground("light-dark(white, #0a0a0a)"),
        )
        .name(
            "assistant",
            Look::at(Position::Left).background("light-dark(white, #262626)"),
        )
        .name("card", Look::at(Position::Left).background("transparent"))
}

/// What the transcript and every bubble drawn here render: everything
/// but images, whose alt text stands in, so a message fetches nothing.
fn options() -> RenderOptions {
    RenderOptions {
        images: false,
        ..Default::default()
    }
}

/// Whether a link's destination is the system browser's to open.
fn is_web(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The tab's state: the transcript, the messages drawn from it, the text
/// of the reply still arriving, and whether a turn is in flight.
#[derive(Clone, Copy)]
struct Window {
    transcript: StoredValue<Transcript>,
    messages: RwSignal<Vec<Message>>,
    /// The one live reply's text. A transcript has one live entry at a
    /// time, its tail, so one signal serves them all.
    live: RwSignal<String>,
    /// Set on send, cleared by the item that ends the turn.
    busy: RwSignal<bool>,
    on_link: Callback<String>,
}

impl Window {
    fn new(on_link: Callback<String>) -> Self {
        Self {
            transcript: StoredValue::new(Transcript::default()),
            messages: RwSignal::new(Vec::new()),
            live: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            on_link,
        }
    }

    /// Folds one arriving item into the transcript and applies what
    /// changed.
    fn fold(self, item: TranscriptItem) {
        if item.ends_turn() {
            self.busy.set(false);
        }
        if let Some(change) = self
            .transcript
            .try_update_value(|transcript| transcript.fold(item))
        {
            self.apply(change);
        }
    }

    fn apply(self, change: Change) {
        match change {
            Change::Append(entry) => {
                let message = self.message(entry);
                self.messages.update(|messages| messages.push(message));
            }
            Change::Grow { delta, .. } => self.live.update(|text| text.push_str(&delta)),
            Change::Replace { id, with } => {
                let message = self.message(with);
                self.messages.update(|messages| {
                    if let Some(shown) = messages.iter_mut().find(|shown| shown.id == id) {
                        *shown = message;
                    }
                });
            }
        }
    }

    /// An entry as `Chat` shows it.
    fn message(self, Entry { id, body }: Entry) -> Message {
        match body {
            Body::Speech {
                role: Role::User,
                text,
                ..
            } => Message::new(id, "user", text),
            Body::Speech {
                role: Role::Assistant,
                text,
                live: true,
            } => {
                self.live.set(text);
                Message::streaming(id, "assistant", self.live)
            }
            Body::Speech {
                role: Role::Assistant,
                text,
                live: false,
            } => {
                let segments = segments(&text);
                if matches!(segments[..], [] | [Segment::Markdown(_)]) {
                    Message::new(id, "assistant", text)
                } else {
                    let on_link = self.on_link;
                    Message::view(id, "assistant", move || bubble(&segments, on_link))
                }
            }
            Body::Card(card) => {
                Message::view(id, "card", move || view! { <Card card=card.clone() /> })
            }
            Body::Question { id: question, ask } => Message::view(id, "card", move || {
                view! { <QuestionCard id=question.clone() ask=ask.clone() /> }
            }),
        }
    }
}

/// What a JSON tree wears inside a bubble: the code blocks' look.
const TREE: &str = "rc-code my-2 overflow-x-auto rounded-[9px] border border-(--rc-border)";

/// A finished reply that holds JSON documents: a bubble of its Markdown
/// runs with a folding tree for each document between them.
fn bubble(segments: &[Segment], on_link: Callback<String>) -> impl IntoView + use<> {
    let pieces = segments
        .iter()
        .cloned()
        .map(|segment| match segment {
            Segment::Markdown(text) => view! {
                <RichText content=Signal::stored(text) options=options() on_link=on_link />
            }
            .into_any(),
            Segment::Json(text) => view! {
                <pre class=TREE>
                    <JsonTree text />
                </pre>
            }
            .into_any(),
        })
        .collect_view();
    view! { <div class="rc-bubble">{pieces}</div> }
}

/// The assistant tab: the theme toggle over the conversation.
#[component]
pub fn Assistant() -> impl IntoView {
    // Links open only the system browser: the webview never navigates.
    let on_link = Callback::new(|url: String| {
        if is_web(&url) {
            ipc::open_url(url);
        }
    });
    let tab = Window::new(on_link);

    // History first, then live items, through the same fold.
    spawn_local(async move {
        for item in ipc::get_transcript().await {
            tab.fold(item);
        }
    });
    ipc::listen_transcript(move |item| tab.fold(item));

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

    let send = move |text: String| {
        tab.busy.set(true);
        spawn_local(async move {
            // A refusal from the command channel: the send never
            // started, so the failure that ends the turn is folded here.
            if let Err(reason) = ipc::send_message(text).await {
                tab.fold(TranscriptItem::TurnFailed { reason });
            }
        });
    };

    view! {
        <RichChatStyle names=names() />
        <main class="flex h-full flex-col bg-neutral-50 dark:bg-neutral-900">
            <header class="flex shrink-0 items-center justify-end border-b border-neutral-200 px-3 py-1 dark:border-neutral-800">
                <button
                    type="button"
                    aria-label="Switch between light and dark"
                    class="rounded-md p-1.5 text-neutral-500 hover:bg-neutral-200 hover:text-neutral-700 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
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
            </header>
            <div class="min-h-0 flex-1">
                <Chat
                    messages=tab.messages
                    on_send=send
                    busy=tab.busy
                    busy_label="Waiting for a reply"
                    placeholder="Message"
                    send=|| {
                        view! {
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
                        }
                    }
                    on_link=on_link
                    options=options()
                />
            </div>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use epik_core::assistant::{Answer, Ask};
    use leptos_rich_chat::Body as Drawn;

    use super::*;

    fn window() -> Window {
        Window::new(Callback::new(|_| {}))
    }

    fn said(role: Role, text: &str) -> TranscriptItem {
        TranscriptItem::Message {
            role,
            text: text.to_owned(),
        }
    }

    fn delta(text: &str) -> TranscriptItem {
        TranscriptItem::AssistantDelta {
            text: text.to_owned(),
        }
    }

    fn where_to() -> Ask {
        Ask::Repository {
            prompt: "Where should this live?".to_owned(),
        }
    }

    /// Each message's name, whether it is live, and its text — `None`
    /// for a body drawn here.
    fn shown(window: Window) -> Vec<(String, bool, Option<String>)> {
        window
            .messages
            .get_untracked()
            .into_iter()
            .map(|message| {
                let text = match message.body {
                    Drawn::Text(text) => Some(text.get_untracked()),
                    Drawn::View(_) => None,
                };
                (message.name, message.live, text)
            })
            .collect()
    }

    fn text(name: &str, live: bool, text: &str) -> (String, bool, Option<String>) {
        (name.to_owned(), live, Some(text.to_owned()))
    }

    fn drawn(name: &str) -> (String, bool, Option<String>) {
        (name.to_owned(), false, None)
    }

    #[test]
    fn speech_is_a_text_message_under_its_speakers_name() {
        let window = window();
        window.fold(said(Role::User, "hi"));
        window.fold(said(Role::Assistant, "**Hello**"));
        assert_eq!(
            shown(window),
            [
                text("user", false, "hi"),
                text("assistant", false, "**Hello**")
            ]
        );
    }

    #[test]
    fn a_reply_streams_through_the_live_signal_and_settles_in_place() {
        let window = window();
        window.fold(said(Role::User, "hi"));
        window.fold(delta("Hel"));
        window.fold(delta("lo"));
        assert_eq!(
            shown(window),
            [text("user", false, "hi"), text("assistant", true, "Hello")]
        );
        let streaming = window.messages.get_untracked()[1].id.clone();

        window.fold(said(Role::Assistant, "Hello."));
        assert_eq!(
            shown(window),
            [
                text("user", false, "hi"),
                text("assistant", false, "Hello.")
            ]
        );
        assert_ne!(
            window.messages.get_untracked()[1].id,
            streaming,
            "a fresh id, so the transcript rebuilds the bubble"
        );
    }

    #[test]
    fn a_second_reply_starts_the_live_signal_over() {
        let window = window();
        window.fold(delta("first"));
        window.fold(said(Role::Assistant, "first"));
        window.fold(delta("second"));
        assert_eq!(window.live.get_untracked(), "second");
    }

    #[test]
    fn a_reply_holding_a_json_document_is_drawn_here() {
        let window = window();
        window.fold(said(
            Role::Assistant,
            "Here:\n\n```json\n{\"a\": 1}\n```\n\nDone.",
        ));
        window.fold(said(Role::Assistant, "```json\n{\"a\": 1}\n```"));
        assert_eq!(shown(window), [drawn("assistant"), drawn("assistant")]);
    }

    #[test]
    fn a_json_fence_that_is_no_document_stays_markdown() {
        let reply = "```json\n// a comment\n{\"a\": 1}\n```";
        let window = window();
        window.fold(said(Role::Assistant, reply));
        assert_eq!(shown(window), [text("assistant", false, reply)]);
    }

    #[test]
    fn an_empty_reply_is_an_empty_bubble() {
        let window = window();
        window.fold(said(Role::Assistant, ""));
        assert_eq!(shown(window), [text("assistant", false, "")]);
    }

    #[test]
    fn acts_and_questions_are_cards() {
        let window = window();
        window.fold(TranscriptItem::ToolCall {
            name: "current_time".to_owned(),
            arguments: "{}".to_owned(),
        });
        window.fold(TranscriptItem::Question {
            id: "1".to_owned(),
            ask: where_to(),
        });
        window.fold(TranscriptItem::TurnFailed {
            reason: "no key".to_owned(),
        });
        assert_eq!(shown(window), [drawn("card"), drawn("card"), drawn("card")]);
    }

    #[test]
    fn a_settled_question_takes_its_questions_place() {
        let window = window();
        window.fold(TranscriptItem::Question {
            id: "1".to_owned(),
            ask: where_to(),
        });
        window.fold(said(Role::User, "meanwhile"));
        let asked = window.messages.get_untracked()[0].id.clone();

        window.fold(TranscriptItem::QuestionResolved {
            id: "1".to_owned(),
            ask: where_to(),
            answer: Answer::Declined,
        });
        assert_eq!(
            shown(window),
            [drawn("card"), text("user", false, "meanwhile")]
        );
        assert_ne!(window.messages.get_untracked()[0].id, asked);
    }

    #[test]
    fn the_item_that_ends_a_turn_clears_busy() {
        let window = window();
        window.busy.set(true);
        window.fold(delta("Hel"));
        assert!(window.busy.get_untracked(), "a reply in progress");
        window.fold(said(Role::Assistant, "Hello"));
        assert!(!window.busy.get_untracked());

        window.busy.set(true);
        window.fold(TranscriptItem::TurnFailed {
            reason: "the wire went quiet".to_owned(),
        });
        assert!(!window.busy.get_untracked());
    }

    #[test]
    fn the_messages_follow_the_transcripts_entries() {
        let window = window();
        window.fold(said(Role::User, "hi"));
        window.fold(delta("Hel"));
        window.fold(said(Role::Assistant, "Hello"));
        let ids: Vec<_> = window
            .messages
            .get_untracked()
            .into_iter()
            .map(|message| message.id)
            .collect();
        let entries: Vec<_> = window.transcript.with_value(|transcript| {
            transcript
                .entries()
                .iter()
                .map(|entry| entry.id.clone())
                .collect()
        });
        assert_eq!(ids, entries);
    }

    #[test]
    fn only_web_links_open() {
        assert!(is_web("https://example.com"));
        assert!(is_web("http://example.com"));
        assert!(!is_web("mailto:someone@example.com"));
        assert!(!is_web("file:///etc/passwd"));
    }

    #[test]
    fn images_are_never_fetched() {
        assert!(!options().images);
    }
}
