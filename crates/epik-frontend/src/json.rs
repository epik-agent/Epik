//! The one JSON renderer in the window: a folding, colored tree for
//! tool-call arguments, tool results, and fenced ```json blocks alike.
//!
//! [`rows`] is a pure function: (text, the set of folded pointers) →
//! one [`Row`] per rendered line, laid out the way `to_string_pretty`
//! would lay it out, with every span classed from the existing `.hl-*`
//! palette. Fold state is an input, never mutated, which is what makes
//! the folded rendering golden-testable; [`initial_folds`] is the fold
//! policy — nested containers and long strings start folded, the root
//! stays open — and [`JsonTree`] is the one component, owning the fold
//! set as a signal and rendering lines only, so a card body and a
//! `<pre>` block each keep their own chrome.
//!
//! Text that is not a JSON object or array — a scalar, prose, a failure
//! reason — is `None`, and the caller renders its plain block exactly as
//! before; never an error, never a panic, the same contract
//! [`highlight::highlight`] keeps. A tool result the library elided at
//! `TOOL_RESULT_CAP` arrives as a truncated document; that is exactly
//! what folding is for, so it is repaired here — cut at the last value
//! boundary, closed, and marked truncated — as a rendering concern the
//! library need not know about.
//!
//! [`highlight::highlight`]: crate::highlight::highlight

use std::collections::HashSet;

use leptos::prelude::*;
use serde_json::Value;

/// One rendered line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    /// Indent level; 0 is the outermost container's brace.
    pub depth: usize,
    /// Left-to-right tokens on this line.
    pub spans: Vec<Span>,
    /// Present when this line can be folded: a JSON Pointer naming what
    /// folds, and whether it currently is.
    pub fold: Option<Fold>,
}

/// One token on a line: its class from the `.hl-*` palette, or `None`
/// for plain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    pub class: Option<&'static str>,
    pub text: String,
}

/// What a foldable line folds: the RFC 6901 pointer of the container or
/// long string, and whether it is currently folded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fold {
    pub pointer: String,
    pub folded: bool,
}

/// The palette, by role.
const KEY: &str = "hl-ty";
const STRING: &str = "hl-st";
const NUMBER: &str = "hl-nm";
const LITERAL: &str = "hl-kw";
const NOTE: &str = "hl-cm";

/// A string value longer than this many characters folds on its own
/// row: unfolded in full, folded to its first `VALUE_CAP` characters
/// and an ellipsis.
pub const VALUE_CAP: usize = 120;

/// The most rows one document renders. Past it, one final row counts
/// the rest — a pathological payload degrades instead of freezing the
/// window.
pub const MAX_ROWS: usize = 2000;

/// The lines of `text` as a tree with `folded` pointers folded — or
/// `None` when `text` is not a JSON object or array, even after repair.
pub fn rows(text: &str, folded: &HashSet<String>) -> Option<Vec<Row>> {
    let (value, truncated) = parse(text)?;
    let mut walk = Walk {
        folded,
        rows: Vec::new(),
        total: 0,
    };
    walk.value(&value, None, String::new(), 0, false);
    let mut rows = walk.rows;
    if walk.total > MAX_ROWS {
        rows.push(Row {
            depth: 0,
            spans: vec![plain(format!("… ({} more lines)", walk.total - MAX_ROWS))],
            fold: None,
        });
    }
    if truncated {
        rows.push(Row {
            depth: 0,
            spans: vec![classed(NOTE, "… (result truncated)")],
            fold: None,
        });
    }
    Some(rows)
}

/// The fold policy for a fresh rendering of `text`: every non-empty
/// container at depth ≥ 1, and every string longer than [`VALUE_CAP`].
/// The root stays open. Empty when the text does not parse.
pub fn initial_folds(text: &str) -> HashSet<String> {
    let mut folds = HashSet::new();
    if let Some((value, _)) = parse(text) {
        collect_folds(&value, String::new(), 0, &mut folds);
    }
    folds
}

fn collect_folds(value: &Value, pointer: String, depth: usize, folds: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            if depth >= 1 && !map.is_empty() {
                folds.insert(pointer.clone());
            }
            for (key, child) in map {
                collect_folds(
                    child,
                    format!("{pointer}/{}", escape(key)),
                    depth + 1,
                    folds,
                );
            }
        }
        Value::Array(items) => {
            if depth >= 1 && !items.is_empty() {
                folds.insert(pointer.clone());
            }
            for (index, child) in items.iter().enumerate() {
                collect_folds(child, format!("{pointer}/{index}"), depth + 1, folds);
            }
        }
        Value::String(text) if is_long(text) => {
            folds.insert(pointer);
        }
        _ => {}
    }
}

/// `text` as a document — parsed straight, or repaired from a
/// truncation — with whether repair was needed. `None` for anything
/// that is not an object or array.
fn parse(text: &str) -> Option<(Value, bool)> {
    let (value, truncated) = match serde_json::from_str::<Value>(text) {
        Ok(value) => (value, false),
        Err(_) => (repair(text)?, true),
    };
    // A repair that salvaged nothing — an empty root — is no repair.
    let root = match &value {
        Value::Object(map) => !truncated || !map.is_empty(),
        Value::Array(items) => !truncated || !items.is_empty(),
        _ => false,
    };
    root.then_some((value, truncated))
}

/// A JSON Pointer reference token for `key`: `~` and `/` escaped.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn is_long(text: &str) -> bool {
    text.chars().count() > VALUE_CAP
}

fn plain(text: impl Into<String>) -> Span {
    Span {
        class: None,
        text: text.into(),
    }
}

fn classed(class: &'static str, text: impl Into<String>) -> Span {
    Span {
        class: Some(class),
        text: text.into(),
    }
}

/// The descent: rows out, folds in, and a running total that keeps
/// counting past [`MAX_ROWS`] so the final row can say how much was
/// left out.
struct Walk<'a> {
    folded: &'a HashSet<String>,
    rows: Vec<Row>,
    total: usize,
}

impl Walk<'_> {
    fn push(&mut self, row: Row) {
        self.total += 1;
        if self.total <= MAX_ROWS {
            self.rows.push(row);
        }
    }

    /// The row(s) for `value` at `pointer`, labelled by `key` when it is
    /// an object member, with a trailing comma when `comma`.
    fn value(
        &mut self,
        value: &Value,
        key: Option<&str>,
        pointer: String,
        depth: usize,
        comma: bool,
    ) {
        let mut lead = Vec::new();
        if let Some(key) = key {
            lead.push(classed(KEY, quote(key)));
            lead.push(plain(": "));
        }
        let tail = if comma { "," } else { "" };
        match value {
            Value::Object(map) => {
                self.container(
                    lead,
                    Container::Object(map.len()),
                    pointer,
                    depth,
                    tail,
                    |walk, pointer| {
                        let last = map.len().saturating_sub(1);
                        for (index, (key, child)) in map.iter().enumerate() {
                            walk.value(
                                child,
                                Some(key),
                                format!("{pointer}/{}", escape(key)),
                                depth + 1,
                                index < last,
                            );
                        }
                    },
                );
            }
            Value::Array(items) => {
                self.container(
                    lead,
                    Container::Array(items.len()),
                    pointer,
                    depth,
                    tail,
                    |walk, pointer| {
                        let last = items.len().saturating_sub(1);
                        for (index, child) in items.iter().enumerate() {
                            walk.value(
                                child,
                                None,
                                format!("{pointer}/{index}"),
                                depth + 1,
                                index < last,
                            );
                        }
                    },
                );
            }
            Value::String(text) if is_long(text) => {
                let folded = self.folded.contains(&pointer);
                let literal = quote(text);
                let shown = if folded {
                    let mut head: String = literal.chars().take(VALUE_CAP).collect();
                    head.push('…');
                    head
                } else {
                    literal
                };
                lead.push(classed(STRING, shown));
                lead.push(plain(tail));
                self.push(Row {
                    depth,
                    spans: trimmed(lead),
                    fold: Some(Fold { pointer, folded }),
                });
            }
            Value::String(text) => self.scalar(lead, classed(STRING, quote(text)), depth, tail),
            Value::Number(number) => {
                self.scalar(lead, classed(NUMBER, number.to_string()), depth, tail);
            }
            Value::Bool(_) | Value::Null => {
                self.scalar(lead, classed(LITERAL, value.to_string()), depth, tail);
            }
        }
    }

    fn scalar(&mut self, mut lead: Vec<Span>, span: Span, depth: usize, tail: &str) {
        lead.push(span);
        lead.push(plain(tail));
        self.push(Row {
            depth,
            spans: trimmed(lead),
            fold: None,
        });
    }

    /// A container's rows: inline when empty, one summary row when
    /// folded, otherwise its opening row, `children`, and its closer.
    fn container(
        &mut self,
        mut lead: Vec<Span>,
        container: Container,
        pointer: String,
        depth: usize,
        tail: &str,
        children: impl FnOnce(&mut Self, &str),
    ) {
        let (open, close, len, summary) = match container {
            Container::Object(len) => ('{', '}', len, format!("…{}", count(len, "key", "keys"))),
            Container::Array(len) => ('[', ']', len, format!("…{}", count(len, "item", "items"))),
        };
        if len == 0 {
            lead.push(plain(format!("{open}{close}{tail}")));
            self.push(Row {
                depth,
                spans: lead,
                fold: None,
            });
            return;
        }
        let folded = self.folded.contains(&pointer);
        if folded {
            lead.push(plain(open));
            lead.push(classed(NOTE, summary));
            lead.push(plain(format!("{close}{tail}")));
            self.push(Row {
                depth,
                spans: lead,
                fold: Some(Fold {
                    pointer,
                    folded: true,
                }),
            });
            return;
        }
        lead.push(plain(open));
        self.push(Row {
            depth,
            spans: lead,
            fold: Some(Fold {
                pointer: pointer.clone(),
                folded: false,
            }),
        });
        children(self, &pointer);
        self.push(Row {
            depth,
            spans: vec![plain(format!("{close}{tail}"))],
            fold: None,
        });
    }
}

/// A container by kind and size, for the row that opens or summarizes it.
enum Container {
    Object(usize),
    Array(usize),
}

/// `spans` without an empty trailing span — the tail when there is no
/// comma.
fn trimmed(mut spans: Vec<Span>) -> Vec<Span> {
    if spans.last().is_some_and(|span| span.text.is_empty()) {
        spans.pop();
    }
    spans
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `text` as a JSON string literal, quotes and escapes included.
fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("{text:?}"))
}

/// Repairs a document truncated mid-way — the library's elide, chiefly:
/// scan to the last offset at which the document stood at a value
/// boundary inside an open container, cut there, drop a trailing comma,
/// close what is open, and parse that. `None` when nothing salvageable
/// remains.
fn repair(text: &str) -> Option<Value> {
    #[derive(Clone, Copy, PartialEq)]
    enum Frame {
        /// An object, and whether the next token is a key.
        Object {
            key_next: bool,
        },
        Array,
    }

    let bytes = text.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    // Where the last value ended (or a container opened) with the stack
    // non-empty.
    let mut boundary: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'{' | b'[' => {
                if let Some(Frame::Object { key_next: true }) = stack.last() {
                    break;
                }
                stack.push(if byte == b'{' {
                    Frame::Object { key_next: true }
                } else {
                    Frame::Array
                });
                i += 1;
                boundary = Some(i);
            }
            b'}' | b']' => {
                let closes = match stack.pop() {
                    Some(Frame::Object { .. }) => byte == b'}',
                    Some(Frame::Array) => byte == b']',
                    None => false,
                };
                if !closes {
                    break;
                }
                i += 1;
                if stack.is_empty() {
                    // The whole document closed; whatever follows is
                    // noise, and the last inner boundary rebuilds it.
                    break;
                }
                boundary = Some(i);
            }
            b',' => {
                match stack.last_mut() {
                    Some(Frame::Object { key_next }) => *key_next = true,
                    Some(Frame::Array) => {}
                    None => break,
                }
                i += 1;
            }
            b':' => {
                match stack.last_mut() {
                    Some(Frame::Object { key_next: false }) => {}
                    _ => break,
                }
                i += 1;
            }
            b'"' => {
                let Some(end) = string_end(bytes, i + 1) else {
                    break;
                };
                i = end;
                match stack.last_mut() {
                    Some(Frame::Object { key_next }) if *key_next => {
                        *key_next = false;
                        // A key alone is no boundary; the colon and value
                        // must follow.
                        continue;
                    }
                    Some(_) => boundary = Some(i),
                    None => break,
                }
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r' | b',' | b'}' | b']')
                {
                    i += 1;
                }
                let token = &text[start..i];
                let scalar = matches!(token, "true" | "false" | "null")
                    || serde_json::from_str::<serde_json::Number>(token).is_ok();
                if !scalar || matches!(stack.last(), Some(Frame::Object { key_next: true }) | None)
                {
                    break;
                }
                boundary = Some(i);
            }
        }
    }
    let cut = boundary?;
    let mut repaired = text[..cut].trim_end().trim_end_matches(',').to_owned();
    // The stack as it stood at the boundary is not tracked separately;
    // rescan the kept prefix for what is still open — cheap, and exact.
    for frame in open_frames(&repaired) {
        repaired.push(match frame {
            b'{' => '}',
            _ => ']',
        });
    }
    serde_json::from_str(&repaired).ok()
}

/// The byte offset just past the closing quote of the string whose
/// contents start at `from`, or `None` when it never closes.
fn string_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// The open containers of a prefix that ends at a value boundary, in
/// order — the closers it needs, reversed.
fn open_frames(prefix: &str) -> Vec<u8> {
    let bytes = prefix.as_bytes();
    let mut stack = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => i = string_end(bytes, i + 1).unwrap_or(bytes.len()),
            open @ (b'{' | b'[') => {
                stack.push(open);
                i += 1;
            }
            b'}' | b']' => {
                stack.pop();
                i += 1;
            }
            _ => i += 1,
        }
    }
    stack.reverse();
    stack
}

/// The tree, rendered as lines only — the caller's chrome around it.
/// Owns the fold set; every disclosure toggles one pointer.
///
/// Clicks are handled once, delegated on the line list, and the pointer
/// rides on the button as `data-pointer`: rows are rebuilt in place by
/// position when the set changes, and attributes are patched on rebuild
/// where a per-row closure would go stale.
#[component]
pub fn JsonTree(text: String) -> impl IntoView {
    let folded = RwSignal::new(initial_folds(&text));
    let text = StoredValue::new(text);
    let toggle = move |event: leptos::ev::MouseEvent| {
        let Some(pointer) = clicked_pointer(&event) else {
            return;
        };
        folded.update(|folds| {
            if !folds.remove(&pointer) {
                folds.insert(pointer);
            }
        });
    };
    view! {
        <div class="overflow-x-auto" on:click=toggle>
            {move || {
                let folds = folded.get();
                rows(&text.get_value(), &folds)
                    .unwrap_or_default()
                    .into_iter()
                    .map(row_view)
                    .collect_view()
            }}
        </div>
    }
}

/// The pointer of the disclosure control a click landed on, if it did.
fn clicked_pointer(event: &leptos::ev::MouseEvent) -> Option<String> {
    use wasm_bindgen::JsCast;
    let target = event.target()?;
    let element = target.dyn_ref::<web_sys::Element>()?;
    let button = element.closest("button[data-pointer]").ok()??;
    button.get_attribute("data-pointer")
}

/// The disclosure control's and its spacer's shared width class.
const GUTTER: &str = "inline-block w-4 shrink-0 text-center";

fn row_view(row: Row) -> AnyView {
    let control = match row.fold {
        Some(Fold { pointer, folded }) => view! {
            <button
                type="button"
                class=format!("{GUTTER} cursor-pointer opacity-70 hover:opacity-100")
                aria-expanded=if folded { "false" } else { "true" }
                data-pointer=pointer
            >
                {if folded { "▸" } else { "▾" }}
            </button>
        }
        .into_any(),
        None => view! { <span class=GUTTER></span> }.into_any(),
    };
    let spans = row
        .spans
        .into_iter()
        .map(|span| match span.class {
            Some(class) => view! { <span class=class>{span.text}</span> }.into_any(),
            None => span.text.into_any(),
        })
        .collect_view();
    view! {
        <div class="whitespace-pre" style:padding-left=format!("{}ch", row.depth * 2)>
            {control}
            {spans}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    fn set(pointers: &[&str]) -> HashSet<String> {
        pointers.iter().map(|&pointer| pointer.to_owned()).collect()
    }

    fn row(depth: usize, spans: Vec<Span>, fold: Option<(&str, bool)>) -> Row {
        Row {
            depth,
            spans,
            fold: fold.map(|(pointer, folded)| Fold {
                pointer: pointer.to_owned(),
                folded,
            }),
        }
    }

    fn key(name: &str) -> Vec<Span> {
        vec![classed(KEY, format!("\"{name}\"")), plain(": ")]
    }

    fn with(mut lead: Vec<Span>, more: Vec<Span>) -> Vec<Span> {
        lead.extend(more);
        lead
    }

    #[test]
    fn a_flat_object_of_scalars_is_one_row_per_field_root_open_no_folds() {
        let text = r#"{"ok": true, "count": 3, "name": "wumpus", "gone": null}"#;
        assert_eq!(initial_folds(text), none());
        assert_eq!(
            rows(text, &none()).unwrap(),
            [
                row(0, vec![plain("{")], Some(("", false))),
                row(
                    1,
                    with(key("count"), vec![classed(NUMBER, "3"), plain(",")]),
                    None
                ),
                row(
                    1,
                    with(key("gone"), vec![classed(LITERAL, "null"), plain(",")]),
                    None
                ),
                row(
                    1,
                    with(key("name"), vec![classed(STRING, "\"wumpus\""), plain(",")]),
                    None
                ),
                row(1, with(key("ok"), vec![classed(LITERAL, "true")]), None),
                row(0, vec![plain("}")], None),
            ]
        );
    }

    #[test]
    fn nested_containers_start_folded_with_singular_and_plural_summaries() {
        let text = r#"{"issues": [{"title": "one"}], "meta": {"page": 1, "of": 2}}"#;
        let folds = initial_folds(text);
        assert_eq!(folds, set(&["/issues", "/issues/0", "/meta"]));
        assert_eq!(
            rows(text, &folds).unwrap(),
            [
                row(0, vec![plain("{")], Some(("", false))),
                row(
                    1,
                    with(
                        key("issues"),
                        vec![plain("["), classed(NOTE, "…1 item"), plain("],")]
                    ),
                    Some(("/issues", true))
                ),
                row(
                    1,
                    with(
                        key("meta"),
                        vec![plain("{"), classed(NOTE, "…2 keys"), plain("}")]
                    ),
                    Some(("/meta", true))
                ),
                row(0, vec![plain("}")], None),
            ]
        );
    }

    #[test]
    fn unfolding_one_pointer_expands_only_that_container() {
        let text = r#"{"issues": [{"title": "one"}], "meta": {"page": 1}}"#;
        let folds = set(&["/issues/0", "/meta"]);
        assert_eq!(
            rows(text, &folds).unwrap(),
            [
                row(0, vec![plain("{")], Some(("", false))),
                row(
                    1,
                    with(key("issues"), vec![plain("[")]),
                    Some(("/issues", false))
                ),
                row(
                    2,
                    vec![plain("{"), classed(NOTE, "…1 key"), plain("}")],
                    Some(("/issues/0", true))
                ),
                row(1, vec![plain("],")], None),
                row(
                    1,
                    with(
                        key("meta"),
                        vec![plain("{"), classed(NOTE, "…1 key"), plain("}")]
                    ),
                    Some(("/meta", true))
                ),
                row(0, vec![plain("}")], None),
            ]
        );
    }

    #[test]
    fn empty_containers_render_inline_and_never_fold() {
        let text = r#"{"a": {}, "b": []}"#;
        assert_eq!(initial_folds(text), none());
        assert_eq!(
            rows(text, &none()).unwrap(),
            [
                row(0, vec![plain("{")], Some(("", false))),
                row(1, with(key("a"), vec![plain("{},")]), None),
                row(1, with(key("b"), vec![plain("[]")]), None),
                row(0, vec![plain("}")], None),
            ]
        );
        assert_eq!(
            rows("[]", &none()).unwrap(),
            [row(0, vec![plain("[]")], None)]
        );
    }

    #[test]
    fn keys_with_tildes_and_slashes_produce_escaped_pointers() {
        let text = r#"{"a/b": {"x": 1}, "c~d": [1]}"#;
        assert_eq!(initial_folds(text), set(&["/a~1b", "/c~0d"]));
        let shown = rows(text, &none()).unwrap();
        assert_eq!(shown[1].fold.as_ref().unwrap().pointer, "/a~1b");
        assert_eq!(shown[4].fold.as_ref().unwrap().pointer, "/c~0d");
    }

    #[test]
    fn a_string_past_the_cap_folds_and_unfolds() {
        let long = "x".repeat(VALUE_CAP + 1);
        let text = format!(r#"{{"diff": "{long}", "short": "y"}}"#);
        assert_eq!(initial_folds(&text), set(&["/diff"]));

        let folded = rows(&text, &set(&["/diff"])).unwrap();
        let expected = format!("\"{}…", "x".repeat(VALUE_CAP - 1));
        assert_eq!(
            folded[1],
            row(
                1,
                with(key("diff"), vec![classed(STRING, expected), plain(",")]),
                Some(("/diff", true))
            )
        );

        let open = rows(&text, &none()).unwrap();
        assert_eq!(
            open[1],
            row(
                1,
                with(
                    key("diff"),
                    vec![classed(STRING, format!("\"{long}\"")), plain(",")]
                ),
                Some(("/diff", false))
            )
        );

        let exact = format!(r#"{{"s": "{}"}}"#, "x".repeat(VALUE_CAP));
        assert_eq!(initial_folds(&exact), none(), "at the cap is not past it");
        assert_eq!(rows(&exact, &none()).unwrap()[1].fold, None);
    }

    #[test]
    fn scalars_empty_text_and_garbage_are_none() {
        for text in [
            "42",
            "\"no such day\"",
            "true",
            "null",
            "",
            "   ",
            "not json",
            "}",
        ] {
            assert_eq!(rows(text, &none()), None, "{text:?}");
            assert_eq!(initial_folds(text), none(), "{text:?}");
        }
    }

    /// The library elides past TOOL_RESULT_CAP with a note; the cut lands
    /// wherever it lands — here inside a string in a nested object.
    #[test]
    fn a_body_truncated_by_the_elide_is_repaired_and_marked() {
        let text = "{\n  \"issues\": [\n    {\n      \"number\": 1,\n      \"title\": \"first\"\n    },\n    {\n      \"number\": 2,\n      \"title\": \"sec\n… (1234 more characters elided)";
        let shown = rows(text, &none()).unwrap();
        assert_eq!(
            shown,
            [
                row(0, vec![plain("{")], Some(("", false))),
                row(
                    1,
                    with(key("issues"), vec![plain("[")]),
                    Some(("/issues", false))
                ),
                row(2, vec![plain("{")], Some(("/issues/0", false))),
                row(
                    3,
                    with(key("number"), vec![classed(NUMBER, "1"), plain(",")]),
                    None
                ),
                row(
                    3,
                    with(key("title"), vec![classed(STRING, "\"first\"")]),
                    None
                ),
                row(2, vec![plain("},")], None),
                row(2, vec![plain("{")], Some(("/issues/1", false))),
                row(3, with(key("number"), vec![classed(NUMBER, "2")]), None),
                row(2, vec![plain("}")], None),
                row(1, vec![plain("]")], None),
                row(0, vec![plain("}")], None),
                row(0, vec![classed(NOTE, "… (result truncated)")], None),
            ]
        );
        assert_eq!(
            initial_folds(text),
            set(&["/issues", "/issues/0", "/issues/1"])
        );
    }

    #[test]
    fn a_cut_after_a_key_or_a_colon_falls_back_to_the_previous_value() {
        for text in [
            r#"{"a": 1, "b""#,
            r#"{"a": 1, "b":"#,
            r#"{"a": 1, "b": "#,
            "{\"a\": 1, \"b\": tru\n… (3 more characters elided)",
        ] {
            let shown = rows(text, &none()).unwrap();
            assert_eq!(
                shown,
                [
                    row(0, vec![plain("{")], Some(("", false))),
                    row(1, with(key("a"), vec![classed(NUMBER, "1")]), None),
                    row(0, vec![plain("}")], None),
                    row(0, vec![classed(NOTE, "… (result truncated)")], None),
                ],
                "{text:?}"
            );
        }
    }

    #[test]
    fn text_truncated_beyond_repair_is_none() {
        for text in [
            "{\"a\n… (99 more characters elided)",
            "{\"a\": ",
            "[\"open",
            "{",
            "… (99 more characters elided)",
        ] {
            assert_eq!(rows(text, &none()), None, "{text:?}");
        }
    }

    #[test]
    fn a_complete_document_followed_by_the_elide_note_is_kept_whole() {
        let text = "{\"a\": 1}\n… (5 more characters elided)";
        let shown = rows(text, &none()).unwrap();
        assert_eq!(shown.len(), 4);
        assert_eq!(shown[3].spans, [classed(NOTE, "… (result truncated)")]);
    }

    #[test]
    fn past_max_rows_the_tail_is_one_counting_row() {
        let items: Vec<String> = (0..(MAX_ROWS + 10)).map(|i| i.to_string()).collect();
        let text = format!("[{}]", items.join(","));
        let shown = rows(&text, &none()).unwrap();
        assert_eq!(shown.len(), MAX_ROWS + 1);
        // MAX_ROWS + 10 items, plus the opening and closing rows, minus
        // the MAX_ROWS shown.
        assert_eq!(
            shown[MAX_ROWS],
            row(0, vec![plain(format!("… ({} more lines)", 12))], None)
        );
        assert!(
            shown[MAX_ROWS - 1].spans[0]
                .text
                .starts_with(&(MAX_ROWS - 2).to_string())
        );
    }

    #[test]
    fn unicode_keys_and_values_render_and_fold_without_panics() {
        let long = "é".repeat(VALUE_CAP + 5);
        let text = format!(r#"{{"名前": "wümpus", "长": "{long}", "🐍": {{"k": "v"}}}}"#);
        let folds = initial_folds(&text);
        assert_eq!(folds, set(&["/长", "/🐍"]));
        let shown = rows(&text, &folds).unwrap();
        assert_eq!(shown[1].spans[0], classed(KEY, "\"名前\""));
        assert_eq!(shown[1].spans[2], classed(STRING, "\"wümpus\""));
        let folded_value = &shown[2].spans[2].text;
        assert_eq!(folded_value.chars().count(), VALUE_CAP + 1);
        assert!(folded_value.ends_with('…'));

        // A truncation cutting through a multibyte string.
        let cut = "{\"a\": 1, \"b\": \"日本語\n… (9 more characters elided)";
        assert_eq!(rows(cut, &none()).unwrap().len(), 4);
        let cut_key = "{\"a\": 1, \"日本\n… (9 more characters elided)";
        assert_eq!(rows(cut_key, &none()).unwrap().len(), 4);
    }

    #[test]
    fn a_top_level_array_of_objects_folds_each_object() {
        let text = r#"[{"a": 1}, {"b": 2}]"#;
        assert_eq!(initial_folds(text), set(&["/0", "/1"]));
        assert_eq!(
            rows(text, &initial_folds(text)).unwrap(),
            [
                row(0, vec![plain("[")], Some(("", false))),
                row(
                    1,
                    vec![plain("{"), classed(NOTE, "…1 key"), plain("},")],
                    Some(("/0", true))
                ),
                row(
                    1,
                    vec![plain("{"), classed(NOTE, "…1 key"), plain("}")],
                    Some(("/1", true))
                ),
                row(0, vec![plain("]")], None),
            ]
        );
    }
}
