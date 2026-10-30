//! What an agent is doing, as it does it: the stream of its log on the run
//! page, which sticks to the newest line unless you scroll up.

use epik_view::{Change, Entry, Excerpt, LineRange, Stream, StreamEntry};
use leptos::ev::MouseEvent;
use leptos::html::Div;
use leptos::prelude::*;

/// How near the bottom, in pixels, still counts as at the newest line.
const NEAR: i32 = 4;

/// The offset of local time from UTC, in milliseconds, at `at`.
#[cfg(target_arch = "wasm32")]
fn offset(at: i64) -> i64 {
    use leptos::wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        type Date;
        #[wasm_bindgen(constructor)]
        fn new(at: f64) -> Date;
        #[wasm_bindgen(method, js_name = getTimezoneOffset)]
        fn timezone_offset(this: &Date) -> f64;
    }
    -(Date::new(at as f64).timezone_offset() as i64) * 60_000
}

/// Outside a browser, as in tests, times are UTC.
#[cfg(not(target_arch = "wasm32"))]
fn offset(_at: i64) -> i64 {
    0
}

/// `at`, in milliseconds since the Unix epoch, as a local time of day:
/// "18:11:41".
pub fn clock(at: i64) -> String {
    let seconds = (at + offset(at)).div_euclid(1000).rem_euclid(24 * 60 * 60);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// "… 40 more lines".
fn more_lines(more: usize) -> String {
    if more == 1 {
        "… 1 more line".to_owned()
    } else {
        format!("… {more} more lines")
    }
}

/// Whether a line of output says something failed, which shows in red.
fn failed(line: &str) -> bool {
    line.contains("FAILED")
        || line.contains("panicked at")
        || line.starts_with("error")
        || line.trim_end().ends_with(" failed")
}

/// An excerpt's lines, each drawn by `line`, with "… N more lines"
/// between its first and last.
fn excerpt<T>(excerpt: Excerpt<T>, line: impl Fn(T) -> AnyView) -> AnyView {
    let more =
        (excerpt.more > 0).then(|| view! { <div class="more">{more_lines(excerpt.more)}</div> });
    view! {
        {excerpt.head.into_iter().map(&line).collect_view()}
        {more}
        {excerpt.tail.into_iter().map(&line).collect_view()}
    }
    .into_any()
}

/// Lines of output or of a file, under what made them, unless there are
/// none.
fn output<T>(lines: Excerpt<T>, line: impl Fn(T) -> AnyView) -> Option<AnyView> {
    (!lines.head.is_empty() || lines.more > 0)
        .then(|| view! { <div class="output">{excerpt(lines, line)}</div> }.into_any())
}

fn printed(line: String) -> AnyView {
    if failed(&line) {
        view! { <div class="failed">{line}</div> }.into_any()
    } else {
        view! { <div>{line}</div> }.into_any()
    }
}

fn changed(change: Change) -> AnyView {
    match change {
        Change::Removed(line) => view! { <div class="removed">{format!("- {line}")}</div> },
        Change::Added(line) => view! { <div class="added">{format!("+ {line}")}</div> },
    }
    .into_any()
}

/// The agent's words: a paragraph for each, with what is in backticks in
/// monospace.
fn narration(text: &str) -> AnyView {
    text.split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| {
            let spans = paragraph
                .split('`')
                .enumerate()
                .map(|(i, span)| {
                    if i % 2 == 1 {
                        view! { <code>{span.to_owned()}</code> }.into_any()
                    } else {
                        span.to_owned().into_any()
                    }
                })
                .collect_view();
            view! { <p>{spans}</p> }
        })
        .collect_view()
        .into_any()
}

/// " (lines 30-70)".
fn range(LineRange { from, to }: LineRange) -> String {
    match to {
        None => format!(" (from line {from})"),
        Some(to) if to == from => format!(" (line {from})"),
        Some(to) => format!(" (lines {from}-{to})"),
    }
}

/// " · 2 matches: store.rs:88, upload.rs:41".
fn found(matches: usize, hits: Excerpt) -> String {
    let count = match matches {
        0 => "no matches".to_owned(),
        1 => "1 match".to_owned(),
        n => format!("{n} matches"),
    };
    let more = (hits.more > 0).then(|| format!("… {} more", hits.more));
    let hits: Vec<String> = hits.head.into_iter().chain(more).chain(hits.tail).collect();
    if hits.is_empty() {
        format!(" · {count}")
    } else {
        format!(" · {count}: {}", hits.join(", "))
    }
}

/// A path in monospace, after what was done to it.
fn path(done: &'static str, path: String) -> impl IntoView {
    view! { {done}" "<code>{path}</code> }
}

/// One thing the agent said or did, with its time at the left.
fn entry(entry: StreamEntry) -> impl IntoView {
    let (class, what) = match entry.what {
        Entry::Narration { text } => ("narration", narration(&text)),
        Entry::Command {
            command,
            output: printed_lines,
        } => (
            "command",
            view! {
                <div class="command-line"><span class="prompt">"$ "</span>{command}</div>
                {output(printed_lines, printed)}
            }
            .into_any(),
        ),
        Entry::Read { path: read, lines } => (
            "read",
            view! { {path("Read", read)}{lines.map(range)} }.into_any(),
        ),
        Entry::Search {
            pattern,
            within,
            matches,
            hits,
        } => {
            let within = within.map(|within| view! { " in "<code>{within}</code> });
            (
                "search",
                view! {
                    "Searched for "<code>{format!("\"{pattern}\"")}</code>
                    {within}
                    {found(matches, hits)}
                }
                .into_any(),
            )
        }
        Entry::Edit {
            path: edited,
            changes,
        } => (
            "edit",
            view! {
                <div class="path">{path("Edited", edited)}</div>
                {output(changes, changed)}
            }
            .into_any(),
        ),
        Entry::NewFile {
            path: created,
            content,
        } => (
            "new_file",
            view! {
                <div class="path">{path("Created", created)}</div>
                {output(content, |line| view! { <div>{line}</div> }.into_any())}
            }
            .into_any(),
        ),
        Entry::Other { text } => ("other", text.into_any()),
    };
    view! {
        <div class=format!("entry {class}")>
            <time>{entry.at.map(clock)}</time>
            <div class="what">{what}</div>
        </div>
    }
}

/// The stream of an agent's log: each thing it said or did, and a
/// blinking cursor while it is still working. It sticks to the newest
/// line; scrolled up, it holds its place and offers "↓ Latest", which
/// scrolls back down and sticks again.
#[component]
pub fn StreamView(#[prop(into)] stream: Signal<Option<Stream>>) -> impl IntoView {
    let scroller = NodeRef::<Div>::new();
    let stuck = RwSignal::new(true);
    let to_latest = move || {
        if let Some(scroller) = scroller.get_untracked() {
            scroller.set_scroll_top(scroller.scroll_height());
        }
    };
    Effect::new(move || {
        stream.track();
        if stuck.get_untracked() {
            to_latest();
        }
    });
    let scrolled = move |_| {
        if let Some(scroller) = scroller.get_untracked() {
            let below = scroller.scroll_height() - scroller.scroll_top() - scroller.client_height();
            stuck.set(below <= NEAR);
        }
    };
    let latest = move |_: MouseEvent| {
        stuck.set(true);
        to_latest();
    };
    let entries = move || match stream.get() {
        Some(stream) if !stream.entries.is_empty() => {
            let cursor = stream
                .working
                .then(|| view! { <span class="cursor"></span> });
            view! {
                {stream.entries.into_iter().map(entry).collect_view()}
                {cursor}
            }
            .into_any()
        }
        _ => view! { <p class="muted nothing">"Nothing yet."</p> }.into_any(),
    };
    view! {
        <div class="stream-box">
            <div class="stream" node_ref=scroller on:scroll=scrolled>{entries}</div>
            {move || {
                (!stuck.get()).then(|| view! { <button class="latest" on:click=latest>"↓ Latest"</button> })
            }}
        </div>
    }
}
