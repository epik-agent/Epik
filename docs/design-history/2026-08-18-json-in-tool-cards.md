# ADR: One JSON renderer, structural, shared by cards and messages

- **Status:** Accepted, and built
- **Date:** 2026-08-18
- **Source:** Design conversation (Cowork), August 2026
- **Note:** Reconstructed 2026-08-19. The original was delivered as a chat
  attachment and lost before check-in. Rebuilt from session records and checked
  against the shipped implementation on `rewrite`; the decisions are as
  discussed on 08-18, but the wording is not the original's.

## Context

Tool cards printed raw monospace JSON. What was wanted: indentation, syntax
colour, and accordions for long values. Two Rust crates were proposed —
`egui_json_tree` and `json-colorizer` — and alternatives asked for.

## What was already there

The most useful part of the session was finding out how little was missing.
Check before building anything renderer-shaped:

- `epik::chat::TranscriptItem::tool_call` and `tool_result` already run
  `serde_json::to_string_pretty`, verbatim when the text does not parse.
  Indentation was never missing.
- `epik-frontend/src/highlight.rs` is a full syntect highlighter. `"json"` was
  already in `LANGUAGES`, the grammar already vendored, and a seven-class
  `.hl-*` palette already defined for both themes. Its only caller was the
  fenced-code-block arm of `chat.rs`; tool cards never called it.
- Every `epik::tools` handler returns `Result<Value, String>`, so a tool card's
  body is always JSON on the ok path and a plain reason string on the error
  path.

What actually looked bad: `card.rs` rendered one flat monospace `<div>`;
`PREVIEW_CAP` counts **characters**, and pretty-printed JSON spends most of 200
characters on indentation before cutting mid-structure; and expansion was one
all-or-nothing toggle.

## Decisions

### Both proposed crates are rejected, on stack grounds

`egui_json_tree` is an egui widget — immediate-mode painting to canvas or WebGL,
where `epik-frontend` renders to the DOM through Leptos. No adapter exists;
adopting it means a second renderer beside the card family, blind to the `.hl-*`
palette and every Tailwind class, plus a large wasm addition.

`json-colorizer` emits ANSI terminal escape codes. Those are literal garbage
bytes in a webview, and its pretty-printing half is already done by `epik::chat`.

Searching found **no Leptos or DOM JSON-tree crate at all**. A JavaScript viewer
was also rejected: it gives up the pure-Rust, golden-tested rendering discipline
the window is built on.

### Colour comes from structure, not from a grammar

A new module, `epik-frontend/src/json.rs`, with `rows(text, folded) ->
Option<Vec<Row>>` and `initial_folds(text) -> HashSet<String>` over RFC 6901
JSON Pointers. Classes are drawn from the existing palette — key to `hl-ty`,
string to `hl-st`, number to `hl-nm`, bool and null to `hl-kw`, summaries and
elisions to `hl-cm`, punctuation plain — so no CSS changes and keys are
distinguishable from string values.

syntect could not do that. Its JSON grammar scopes a key
`meta.mapping.key.json string.quoted.double.json`, and `classify()` walks the
scope stack innermost-first, so a key and a string value both resolve to
`hl-st`.

Following `card.rs`'s precedent — pure `spec()` and `shown()` beside the `Card`
component — `json.rs` holds both the pure functions and one `JsonTree`
component, and renders **lines only**. Each caller supplies its own chrome.

### Flat rows, with fold state as an input

Not recursive components. The fold set is an argument to a pure function, which
is what makes the *folded* rendering golden-testable rather than only the
expanded one. Same shape as `markdown::parse`, `highlight::highlight`,
`card::shown`, and the Mermaid renderer.

### One JSON renderer for the whole window

The better framing, and the one adopted: could the same code render JSON in
cards and JSON in non-card output? Fenced ` ```json ` blocks in assistant
messages render through the same `JsonTree`, **with fold controls, exactly as
cards do**. Foldable-in-prose was chosen over static after the trade was named.

This deliberately crosses a line the codebase had drawn. `card.rs`'s doc comment
called expand-on-click "the component's only interactivity," and bubbles had
been things someone said rather than things you operate. Crossed on purpose;
that doc comment is stale and this decision corrects it.

Escape hatch, recorded: if controls-in-bubbles reads badly in use, the fix is to
**promote the block to a card** — consistent with the standing direction that
future model-addressed rich cards consume the same `Card` component — not to
remove the controls.

### syntect keeps `"json"`, demoted to fragment fallback

The `Node::CodeBlock` arm cascades: a json-tagged fence whose text `json::rows`
parses renders as a `JsonTree`; otherwise `highlight()`; otherwise plain.

The asymmetry forcing this is that **prose JSON frequently is not JSON**. Models
write `// comment`, `...`, `<your-value-here>`, trailing commas, and brace-less
fragments — all correct as explanation and invalid as documents. A structural
renderer returns `None` on every one of them; a grammar tokenizes them happily.
Cards need no cascade, because tool JSON either parses or is a plain reason
string.

Do not extend the JSON branch to untagged code blocks. Streaming is unaffected:
deltas render plain by standing design.

### Fold policy: collapse below depth 1

The root container is open; every container beneath starts folded, shown as
`{…4 keys}` or `[…12 items]`. This replaces `PREVIEW_CAP` on the JSON path
entirely, so a card's height depends on the payload's **shape** rather than its
size. `PREVIEW_CAP` and `shown()` are untouched for prose and unparseable
machine text.

Long scalars fold too, at `VALUE_CAP = 120`. A four-kilobyte diff in one field
is the motivating case for "accordions for long values."

### The renderer repairs a truncated document rather than refusing it

`TOOL_RESULT_CAP` stays at 2000 and the library's elide stays byte-naive. The
frontend tolerates the truncated tail: `json.rs` scans tracking string state and
bracket depth, cuts to the last value boundary, appends balancing closers,
reparses, and appends a `… (result truncated)` row. Unrepairable text falls to
`None` and today's plain block.

The reading adopted is that the library keeps a faithful prefix of what the
model saw, and repair is a rendering concern. Rejected: making the library's
elide structure-aware.

## Rejected alternatives

- **Carrying `serde_json::Value` across IPC** instead of a `String`. The string
  is what makes "verbatim when it doesn't parse" expressible at all; parsing for
  display is a rendering concern, like parsing markdown.
- **Extending `highlight.rs` to carry structure.** It is a grammar dispatcher
  earning its keep across seventeen languages, and teaching it that JSON is
  special would make it two things.

## Out of scope, named

Copy-to-clipboard, search-within-document, and palette retuning.

## Note

`serde_json` is not in `epik-frontend`'s `[dependencies]` but is unconditional
in `epik`'s and already present in `crates/epik-frontend/Cargo.lock`. Naming it
is a lockfile no-op with zero bundle growth.
