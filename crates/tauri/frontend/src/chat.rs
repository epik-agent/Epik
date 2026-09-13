//! The chat surface: the app's main view.
//!
//! The transcript the user sees is a pure fold over [`TranscriptItem`]s —
//! history from `get_transcript` first, then live events on top. Nothing
//! is shown that didn't come back over the barrier as an item. Message
//! text renders through the deterministic [`markdown`] parse into a
//! typed structure mapped 1:1 into elements — a chat message can never
//! inject markup, links open only the system browser through one
//! delegated handler, and images never fetch.

use epik::chat::{Role, TranscriptItem};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::card::{self, Card, QuestionCard};
use crate::highlight;
use crate::ipc;
use crate::json::{self, JsonTree};
use crate::markdown::{self, Item, Node};

/// Folds one arriving item into the transcript. New item kinds become new
/// arms here, never a second channel.
///
/// Deltas accumulate into one in-progress tail item; the completed
/// message replaces that tail, its text authoritative. A failure also
/// clears the tail — the conversation never held the fragments, so the
/// view doesn't either — and lands as its own item. A resolved question
/// replaces the pending question with its id, or appends when none is
/// pending (a window that mounted mid-question and caught only the
/// resolution) — replace-or-append is the whole rule.
pub fn fold(transcript: &mut Vec<TranscriptItem>, item: TranscriptItem) {
    match item {
        TranscriptItem::QuestionResolved { ref id, .. }
            if let Some(pending) = transcript.iter().position(
                |shown| matches!(shown, TranscriptItem::Question { id: asked, .. } if asked == id),
            ) =>
        {
            transcript[pending] = item;
        }
        TranscriptItem::Message { .. }
        | TranscriptItem::TurnFailed { .. }
        | TranscriptItem::ToolCall { .. }
        | TranscriptItem::ToolResult { .. }
        | TranscriptItem::Question { .. }
        | TranscriptItem::QuestionResolved { .. } => {
            if matches!(
                transcript.last(),
                Some(TranscriptItem::AssistantDelta { .. })
            ) {
                transcript.pop();
            }
            transcript.push(item);
        }
        TranscriptItem::AssistantDelta { text } => {
            if let Some(TranscriptItem::AssistantDelta { text: streaming }) = transcript.last_mut()
            {
                streaming.push_str(&text);
            } else {
                transcript.push(TranscriptItem::AssistantDelta { text });
            }
        }
    }
}

/// What every anchor wears. Clicks are handled once, delegated on the
/// transcript pane — anchors carry no handlers of their own.
const LINK: &str = "cursor-pointer underline decoration-current/50 underline-offset-2 \
                    hover:decoration-current";
/// What inline code wears.
const CODE: &str = "rounded bg-black/10 px-1 font-mono text-[0.9em] dark:bg-white/10";
/// Vertical rhythm for block elements inside a bubble.
const BLOCK: &str = "my-1 first:mt-0 last:mb-0";
/// Table and blockquote edges, both themes.
const EDGE: &str = "border-neutral-300 dark:border-neutral-600";

/// Maps a message's parse 1:1 into the view. Text lands in text nodes —
/// escaped by construction — so markup in a message stays words.
fn render_message(text: &str) -> impl IntoView + use<> {
    nodes_view(&markdown::parse(text))
}

fn nodes_view(nodes: &[Node]) -> AnyView {
    nodes.iter().map(node_view).collect_view().into_any()
}

fn node_view(node: &Node) -> AnyView {
    match node {
        Node::Text(text) => text.clone().into_any(),
        Node::Code(code) => view! { <code class=CODE>{code.clone()}</code> }.into_any(),
        Node::Emphasis(children) => view! { <em>{nodes_view(children)}</em> }.into_any(),
        Node::Strong(children) => view! { <strong>{nodes_view(children)}</strong> }.into_any(),
        Node::Strikethrough(children) => view! { <del>{nodes_view(children)}</del> }.into_any(),
        Node::Link { href, children } => {
            view! { <a href=href.clone() class=LINK>{nodes_view(children)}</a> }.into_any()
        }
        // The image was not fetched; its alt text stands in, dressed to
        // say something was elided.
        Node::Elided(alt) => {
            let alt = if alt.is_empty() {
                "image"
            } else {
                alt.as_str()
            };
            view! { <span class="italic opacity-60">"["{alt.to_owned()}"]"</span> }.into_any()
        }
        Node::Paragraph(children) => view! { <p class=BLOCK>{nodes_view(children)}</p> }.into_any(),
        // Headings scaled for a bubble: a bold lead line, not a
        // billboard.
        Node::Heading { level, children } => {
            let inner = nodes_view(children);
            match level {
                1 => view! { <h1 class=format!("{BLOCK} text-base font-bold")>{inner}</h1> }
                    .into_any(),
                2 => view! { <h2 class=format!("{BLOCK} text-[0.95rem] font-bold")>{inner}</h2> }
                    .into_any(),
                _ => view! { <h3 class=format!("{BLOCK} font-semibold")>{inner}</h3> }.into_any(),
            }
        }
        // The info string's first token picks the rendering, in order:
        // a ```json fence holding a real JSON document is the folding
        // tree; anything the grammar set knows is highlighted — which
        // is where a ```json fence with a comment or an ellipsis in it
        // still lands; a language the pruned set doesn't know renders
        // the same plain block as ever.
        Node::CodeBlock { info, code } => {
            let language = info
                .split([',', ' ', '\t'])
                .next()
                .unwrap_or_default()
                .trim();
            let chrome = format!(
                "{BLOCK} overflow-x-auto rounded-md bg-black/5 p-2 font-mono text-xs dark:bg-white/10"
            );
            if language.eq_ignore_ascii_case("json")
                && json::rows(code, &std::collections::HashSet::new()).is_some()
            {
                return view! {
                    <pre class=chrome>
                        <JsonTree text=code.clone() />
                    </pre>
                }
                .into_any();
            }
            let body = match highlight::highlight(language, code) {
                Some(chunks) => chunks
                    .into_iter()
                    .map(|chunk| match chunk.class {
                        Some(class) => view! { <span class=class>{chunk.text}</span> }.into_any(),
                        None => chunk.text.into_any(),
                    })
                    .collect_view()
                    .into_any(),
                None => code.clone().into_any(),
            };
            view! {
                <pre class=chrome>
                    <code>{body}</code>
                </pre>
            }
            .into_any()
        }
        Node::BlockQuote(children) => view! {
            <blockquote class=format!("{BLOCK} border-l-2 {EDGE} pl-3 opacity-80")>
                {nodes_view(children)}
            </blockquote>
        }
        .into_any(),
        Node::List { start, items } => list_view(*start, items),
        Node::Rule => view! { <hr class=format!("{BLOCK} {EDGE}") /> }.into_any(),
        Node::Table { header, rows } => view! {
            <div class=format!("{BLOCK} overflow-x-auto")>
                <table class="border-collapse text-left">
                    <thead>
                        <tr>
                            {header
                                .iter()
                                .map(|cell| view! {
                                    <th class=format!("border {EDGE} px-2 py-0.5 font-semibold")>
                                        {nodes_view(cell)}
                                    </th>
                                })
                                .collect_view()}
                        </tr>
                    </thead>
                    <tbody>
                        {rows
                            .iter()
                            .map(|row| view! {
                                <tr>
                                    {row
                                        .iter()
                                        .map(|cell| view! {
                                            <td class=format!("border {EDGE} px-2 py-0.5")>
                                                {nodes_view(cell)}
                                            </td>
                                        })
                                        .collect_view()}
                                </tr>
                            })
                            .collect_view()}
                    </tbody>
                </table>
            </div>
        }
        .into_any(),
    }
}

fn list_view(start: Option<u64>, items: &[Item]) -> AnyView {
    // A task list wears checkboxes instead of markers.
    let tasks = items.iter().any(|item| item.checked.is_some());
    let class = if tasks {
        format!("{BLOCK} list-none pl-1")
    } else if start.is_some() {
        format!("{BLOCK} list-decimal pl-5")
    } else {
        format!("{BLOCK} list-disc pl-5")
    };
    let rendered = items
        .iter()
        .map(|item| {
            let checkbox = item.checked.map(|checked| {
                view! {
                    // Disabled on purpose: nothing in a bubble is
                    // interactive except links.
                    <input
                        type="checkbox"
                        disabled=true
                        prop:checked=checked
                        class="mr-1.5 align-middle accent-[#00b377] dark:accent-[#00e599]"
                    />
                }
            });
            view! { <li>{checkbox}{nodes_view(&item.children)}</li> }
        })
        .collect_view();
    match start {
        Some(first) => {
            let start_attr = (first != 1).then(|| first.to_string());
            view! { <ol class=class start=start_attr>{rendered}</ol> }.into_any()
        }
        None => view! { <ul class=class>{rendered}</ul> }.into_any(),
    }
}

/// The delegated link handler: any anchor in the transcript opens the
/// system browser — the webview never navigates. Anchors only exist for
/// http(s) (the parse guarantees it), and the guard here repeats the
/// check anyway.
fn open_link(event: &ev::MouseEvent) {
    let Some(target) = event.target() else { return };
    let Some(element) = target.dyn_ref::<web_sys::Element>() else {
        return;
    };
    let Ok(Some(anchor)) = element.closest("a") else {
        return;
    };
    event.prevent_default();
    if let Some(href) = anchor.get_attribute("href")
        && (href.starts_with("http://") || href.starts_with("https://"))
    {
        ipc::open_url(href);
    }
}

/// What every bubble wears.
const BUBBLE: &str = "max-w-[min(85%,75ch)] rounded-2xl px-3.5 py-2 text-sm leading-relaxed \
                      break-words whitespace-pre-wrap";
/// The assistant's side of the room.
const ASSISTANT: &str = "self-start border border-neutral-200 bg-white text-neutral-900 \
                         dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";

/// One transcript entry: the user against the right edge in the accent,
/// the assistant against the left in neutral — streaming as escaped plain
/// text behind a pulsing caret, completed with the full rendering — and
/// everything that wasn't said — tool calls, tool results, failures,
/// questions — as a [`Card`] in the flow, not a speech bubble; a pending
/// question as the live [`QuestionCard`].
fn bubble(item: &TranscriptItem) -> AnyView {
    match item {
        TranscriptItem::Message { role, text } => {
            let side = match role {
                Role::User => {
                    "self-end bg-[#00b377] text-white dark:bg-[#00e599] dark:text-neutral-950"
                }
                Role::Assistant => ASSISTANT,
            };
            view! { <li class=format!("{BUBBLE} {side}")>{render_message(text)}</li> }.into_any()
        }
        TranscriptItem::AssistantDelta { text } => view! {
            <li class=format!("{BUBBLE} {ASSISTANT}")>
                {text.clone()}
                <span class="ml-0.5 inline-block h-3.5 w-1 animate-pulse rounded-sm bg-current align-text-bottom"></span>
            </li>
        }
        .into_any(),
        TranscriptItem::Question { id, ask } => {
            view! { <QuestionCard id=id.clone() ask=ask.clone() /> }.into_any()
        }
        item => {
            let spec = card::spec(item).expect("every non-speech item is a card");
            view! { <Card spec /> }.into_any()
        }
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

    // Whether a turn is in flight: set on send, cleared by whichever end
    // of the turn arrives — the completed reply or the failure.
    let busy = RwSignal::new(false);

    // History first, then live items, through the same fold.
    spawn_local(async move {
        for item in ipc::get_transcript().await {
            transcript.update(|transcript| fold(transcript, item));
        }
    });
    ipc::listen_transcript(move |item| {
        if matches!(
            &item,
            TranscriptItem::Message {
                role: Role::Assistant,
                ..
            } | TranscriptItem::TurnFailed { .. }
        ) {
            busy.set(false);
        }
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
        if busy.get_untracked() || text.trim().is_empty() {
            return;
        }
        busy.set(true);
        draft.set(String::new());
        if let Some(input) = input.get_untracked() {
            let _ = input.focus();
        }
        spawn_local(async move {
            // A refusal from the command channel — not a turn that
            // failed, but the send never started, so the turn never
            // ends: surface it and stand down.
            if let Err(reason) = ipc::send_message(text).await {
                busy.set(false);
                transcript.update(|transcript| {
                    fold(transcript, TranscriptItem::TurnFailed { reason });
                });
            }
        });
    };

    view! {
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
            <div
                node_ref=pane
                on:click=move |event| open_link(&event)
                on:scroll=move |_| {
                    if let Some(pane) = pane.get_untracked() {
                        let bottom = pane.scroll_top() + pane.client_height();
                        pinned.set(bottom >= pane.scroll_height() - 48);
                    }
                }
                class="min-h-0 flex-1 overflow-y-auto px-4 py-6 [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-neutral-300 dark:[&::-webkit-scrollbar-thumb]:bg-neutral-700"
            >
                <ul class="mx-auto flex max-w-4xl flex-col gap-3">
                    // The key carries kind and length, so the in-progress
                    // bubble re-renders as its text grows and again when
                    // the final message replaces it in place.
                    <For
                        each=move || transcript.get().into_iter().enumerate()
                        key=|(index, item)| {
                            let (kind, length) = match item {
                                TranscriptItem::Message { text, .. } => (0u8, text.len()),
                                TranscriptItem::AssistantDelta { text } => (1, text.len()),
                                TranscriptItem::TurnFailed { reason } => (2, reason.len()),
                                TranscriptItem::ToolCall { arguments, .. } => (3, arguments.len()),
                                TranscriptItem::ToolResult { content, .. } => (4, content.len()),
                                TranscriptItem::Question { id, .. } => (5, id.len()),
                                TranscriptItem::QuestionResolved { id, .. } => (6, id.len()),
                            };
                            (*index, kind, length)
                        }
                        children=|(_, item)| bubble(&item)
                    />
                </ul>
            </div>
            <div class="shrink-0 border-t border-neutral-200 p-3 dark:border-neutral-800">
                <div class="mx-auto flex max-w-4xl items-end gap-2">
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
                        prop:disabled=busy
                        class="shrink-0 rounded-lg bg-[#00b377] p-2 text-white hover:bg-[#009966] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-[#00b377] dark:bg-[#00e599] dark:text-neutral-950 dark:hover:bg-[#33edb3] dark:disabled:hover:bg-[#00e599]"
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

    fn delta(text: &str) -> TranscriptItem {
        TranscriptItem::AssistantDelta {
            text: text.to_owned(),
        }
    }

    #[test]
    fn deltas_accumulate_into_one_in_progress_bubble() {
        let mut transcript = vec![message(Role::User, "hi")];
        fold(&mut transcript, delta("Hel"));
        fold(&mut transcript, delta("lo"));
        assert_eq!(transcript, [message(Role::User, "hi"), delta("Hello")]);
    }

    #[test]
    fn the_completed_message_replaces_the_in_progress_bubble() {
        let mut transcript = vec![message(Role::User, "hi"), delta("Hel")];
        fold(&mut transcript, message(Role::Assistant, "Hello"));
        assert_eq!(
            transcript,
            [message(Role::User, "hi"), message(Role::Assistant, "Hello")],
            "the final text is authoritative"
        );
    }

    #[test]
    fn a_completed_message_with_no_stream_just_lands() {
        let mut transcript = vec![message(Role::User, "hi")];
        fold(&mut transcript, message(Role::Assistant, "Hello"));
        assert_eq!(
            transcript,
            [message(Role::User, "hi"), message(Role::Assistant, "Hello")]
        );
    }

    fn failed(reason: &str) -> TranscriptItem {
        TranscriptItem::TurnFailed {
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn a_failure_clears_the_fragments_and_lands_as_its_own_item() {
        let mut transcript = vec![message(Role::User, "hi"), delta("Hel")];
        fold(&mut transcript, failed("the wire went quiet"));
        assert_eq!(
            transcript,
            [message(Role::User, "hi"), failed("the wire went quiet")],
            "the conversation never held the fragments, so the view doesn't either"
        );
    }

    #[test]
    fn a_failure_with_no_stream_just_lands() {
        let mut transcript = vec![message(Role::User, "hi")];
        fold(&mut transcript, failed("no key"));
        assert_eq!(transcript, [message(Role::User, "hi"), failed("no key")]);
    }

    fn tool_call() -> TranscriptItem {
        TranscriptItem::ToolCall {
            name: "current_time".to_owned(),
            arguments: "{}".to_owned(),
        }
    }

    fn tool_result(ok: bool) -> TranscriptItem {
        TranscriptItem::ToolResult {
            name: "current_time".to_owned(),
            ok,
            content: "{\"local\":\"noon\"}".to_owned(),
        }
    }

    #[test]
    fn tool_items_land_in_order_like_messages() {
        let mut transcript = vec![message(Role::User, "what time is it?")];
        fold(&mut transcript, tool_call());
        fold(&mut transcript, tool_result(true));
        fold(&mut transcript, message(Role::Assistant, "It's noon."));
        assert_eq!(
            transcript,
            [
                message(Role::User, "what time is it?"),
                tool_call(),
                tool_result(true),
                message(Role::Assistant, "It's noon."),
            ]
        );
    }

    /// Text the model streamed before deciding on a tool never became
    /// conversation, so a tool call clears the fragments like a
    /// completed message would.
    #[test]
    fn a_tool_call_clears_a_streaming_tail() {
        let mut transcript = vec![message(Role::User, "hi"), delta("Let me")];
        fold(&mut transcript, tool_call());
        assert_eq!(transcript, [message(Role::User, "hi"), tool_call()]);
    }

    fn where_to() -> epik::chat::Ask {
        epik::chat::Ask::Repository {
            prompt: "Where should this live?".to_owned(),
        }
    }

    fn question(id: &str) -> TranscriptItem {
        TranscriptItem::Question {
            id: id.to_owned(),
            ask: where_to(),
        }
    }

    fn resolved(id: &str) -> TranscriptItem {
        TranscriptItem::QuestionResolved {
            id: id.to_owned(),
            ask: where_to(),
            answer: epik::chat::Answer::Repository {
                url: "/tmp/wumpus.git".to_owned(),
            },
        }
    }

    #[test]
    fn a_question_appends_and_its_resolution_replaces_it_in_place() {
        let mut transcript = vec![message(Role::User, "write me wumpus")];
        fold(&mut transcript, question("1"));
        fold(&mut transcript, tool_call());
        assert_eq!(
            transcript,
            [
                message(Role::User, "write me wumpus"),
                question("1"),
                tool_call()
            ]
        );

        fold(&mut transcript, resolved("1"));
        assert_eq!(
            transcript,
            [
                message(Role::User, "write me wumpus"),
                resolved("1"),
                tool_call()
            ],
            "the resolution takes the pending question's place, not the tail"
        );
    }

    #[test]
    fn a_resolution_with_no_pending_question_just_appends() {
        let mut transcript = vec![message(Role::User, "hi")];
        fold(&mut transcript, resolved("7"));
        assert_eq!(transcript, [message(Role::User, "hi"), resolved("7")]);
    }

    #[test]
    fn a_resolution_replaces_only_the_question_with_its_id() {
        let mut transcript = vec![question("1"), question("2")];
        fold(&mut transcript, resolved("2"));
        assert_eq!(transcript, [question("1"), resolved("2")]);
    }

    #[test]
    fn a_question_clears_a_streaming_tail_like_any_act() {
        let mut transcript = vec![message(Role::User, "hi"), delta("Let me")];
        fold(&mut transcript, question("1"));
        assert_eq!(transcript, [message(Role::User, "hi"), question("1")]);
    }
}
