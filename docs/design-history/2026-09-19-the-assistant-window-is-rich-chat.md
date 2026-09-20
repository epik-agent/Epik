# ADR: The assistant window is leptos-rich-chat, and the assistant lives in epik-core

- **Status:** Accepted
- **Date:** 2026-09-19
- **Source:** Design conversation (Cowork), September 2026

## Context

The window's conversation with the model was a hand-built Leptos surface in
`epik-frontend/src/chat.rs`: its own Markdown parse (`markdown.rs`), its own
pruned syntect highlighter (`highlight.rs`, `build.rs`, `grammars/`), its own
JSON tree (`json.rs`), its own scroll-following and its own text box. The
vocabulary and the model client lived in `epik::chat`, in a crate that
`epik-core` is replacing.

`leptos-rich-chat` (github.com/wpm/Rich-Chat) is a Leptos chat window that
renders Markdown, LaTeX math and highlighted code progressively, follows a
growing transcript, and lets a host draw any entry's body itself
(`Body::View`). The question was how to put Epik's window on it, and where the
logic that is not rendering should live.

The model is called the **assistant** from here on, and names follow.

## Decisions

### The Tauri frontend only renders

Everything that can be stated without a DOM goes to `epik-core::assistant`,
tested natively; the frontend maps it onto components. "Keeping the frontend
minimal" is measured across everything that ships in the wasm bundle, so
moving logic *into* `leptos-rich-chat` does not count as removing it from the
frontend.

### `epik-core::assistant` holds the whole assistant

- **Vocabulary:** `Role`, `TranscriptItem`, `Ask`, `Answer`, `Conversation`,
  `TRANSCRIPT_EVENT`. `TranscriptItem::tool_call` takes a name and arguments
  rather than a `ToolCall`, so the vocabulary depends on no tool type.
- **Transcript:** the fold that was the frontend's, plus identity.
  `leptos-rich-chat` decides what to rebuild by `(id, live)`, so an item needs
  an id that survives it growing and changes when it is replaced.
  `Transcript::fold(item) -> Change` (`Append`, `Grow`, `Replace`, `Remove`);
  every replacement carries a fresh id. An `Entry` is speech, a card, or a
  pending question; the card mapping that was `card::spec` is part of it.
- **JSON:** the pure half of `json.rs` — rows, folds, the depth-1 policy,
  value caps, truncation repair — with spans carrying a semantic kind rather
  than a CSS class. One JSON renderer for cards and messages still holds;
  its logic simply lives in core.
- **Message segments:** a finished assistant message is split into Markdown
  runs and top-level ```json fences that parse as JSON, using pulldown-cmark's
  offset iterator.
- **Client:** `Client`, `run`, `models`, the wire protocol and the system
  prompt, with the tool loop on `epik-core::model_tools::Registry`. `epik-core`
  supersedes `epik`; `epik::chat` keeps re-exports only for what is still in
  `epik`, and a `From<epik::tools::Tool>` bridge lets `epik`'s feature tools
  join the core registry until they move.

### JSON trees in messages come from `Body::View`, not from the library

A finished message with a JSON fence is drawn by Epik as a view body wearing
`rc-bubble`: `RichText` for each Markdown run, the thin `JsonTree` for each
fence. Streaming replies stay `Body::Text`; the split happens when the message
finishes, which is also when `leptos-rich-chat` rebuilds it.

Rejected: a code-fence hook in `leptos-rich-chat`, and moving the JSON tree
into `leptos-rich-chat`. The library already lets the host draw a bubble; both
alternatives grow the library to do what the host can already do. Accepted
cost: Markdown context does not cross a split (a reference definition on the
far side of a fence), and a JSON fence nested in a list or quote is highlighted
rather than folded.

### `busy` is a new composer state, not a redefinition of `disabled`

Epik blocks sending while a turn is in flight and keeps the text box open for
the next message. `leptos-rich-chat`'s `disabled` disables the box. It gains
`busy`: sending blocked, draft kept, box editable, preview live. `disabled`
keeps meaning "the composer is off" — a host with no key wants that — and its
documentation is corrected to say so.

The preview is live while busy because drafting the next message is the reason
to keep the box open. Its costs — a second progressive render beside the
streaming reply, and up to 40vh of height taken from the transcript — are
small, and the preview has its own collapse toggle.

### What the window gives up and gains

- Streaming replies render as Markdown while they arrive, instead of as plain
  text behind a caret.
- Math renders. Currency amounts such as `$0.42 and $1.10` stay text.
- The grammar set is two-face's, not the pruned seventeen, and the math fonts
  are embedded: roughly 1.5 MB more wasm, accepted for a desktop app.
- Links keep going to the system browser through `on_link`, http(s) only;
  images are not fetched (`RenderOptions { images: false }`).

### Styling

`styles.css` declares `@layer theme, base, rich-chat, components, utilities;`
first. Tailwind's preflight must sit beneath `leptos-rich-chat`'s layers or it
strips the rendered Markdown; Epik's utilities sit above so they win. The
`.hl-*` palette goes: code and the JSON tree both take `leptos-rich-chat`'s
highlight classes.

### Names

"chat" becomes "assistant" everywhere except inside the `epik` crate, which is
going away. The one exception outside it is the config key `chat` under
`[model]`: it is defined by `epik::config`, and renaming it would stop existing
config files loading. It is renamed, with an alias, when config moves to core.
