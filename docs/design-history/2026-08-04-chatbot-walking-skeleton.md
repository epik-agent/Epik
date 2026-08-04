# ADR: The window's first skeleton is a chatbot; chat is library infrastructure

- **Status:** Accepted
- **Date:** 2026-08-04
- **Source:** Design conversation (Cowork), August 2026

## Context

The `local-client-greenfield` branch restarts Epik's client as a Rust
library that will underlie both a remote daemon and a Tauri desktop app.
The app-spike (wpm/epik-app-spike, verdict GREEN) proved the interface
architecture — Claude Code driven over stream-json from a Rust host, a
Leptos frontend folding an event stream — but it was chat-first, with a
Claude Code session as its object. The greenfield library's object is a
*run*: a `Feature`, a `Tree<Issue>`, an `Implementable` engine observed
through a serializable event stream.

Theory has outrun Practice. What is needed first is a working window to
use and grow into — and the first thing built in it should carry no domain
weight, so that the permanent infrastructure (app shell, IPC discipline,
streaming, model configuration, secret storage) is forced into existence
by the simplest possible occupant.

Standing constraints from prior decisions: Epik is bound to Anthropic only
through Claude Code and is otherwise model-neutral; tests should be able
to run against a small cheap model; commits made by Epik attribute to the
Epik persona.

## Decision

**The first thing the Epik window does is chat — "Hello, I'm Epik" — and
nothing else.** No git, no issues, no coding. The run cockpit comes
second, into infrastructure the chatbot has already proven.

### Workspace

Three crates: `crates/epik` (the library — domain core and all logic),
`crates/epik-app` (the Tauri 2 host — state mutex, commands, event pump,
and nothing else), `crates/epik-ui` (the Leptos frontend, compiled by
Trunk, outside the workspace because it targets wasm32). Written from
scratch; the spike stays intact as reference.

Structural rules, permanent:

- Types that cross the IPC boundary are library types and stay wasm-clean.
  The event vocabularies move to `event.rs`; the sinks stay in
  `logging.rs`; `Log` generalizes to `Log<E>` so one `Sender` and one
  `JsonLines` implementation serve every vocabulary.
- UI state is a pure fold over the event stream, host-tested. Nothing
  lives in `epik-ui` that is not narrowly concerned with user interface.
- Commands carry intent; events carry observation. Command errors and
  streamed failures are different channels, never mixed.
- The library stays synchronous: blocking I/O and channels, no tokio.
  Async is the host's problem (`spawn_blocking` under Tauri; plain
  threads in the daemon and tests). Cancellation is a stop token checked
  between deltas, which is all a Stop button will ever need.
- **Anything the app can do must be achievable through `epik` library
  calls alone.** This invariant is what keeps every future thin host
  possible — the daemon, the worker, and a Claude-Desktop-facing MCP
  server. Claude Desktop as an alternative UI is not targeted, but this
  is the property that refrains from foreclosing it.

**Frontend stack: Leptos with Tailwind.** Design tokens derive from
`brand.json` (the palette/font/logo source of truth), and the window
ships a **light/dark mode toggle from the very beginning** — theming
retrofits badly, so both themes are exercised from the first component.
The dark palette already defined in `brand.json` (unused by the
light-only website) gets its first consumer here.

### Chat is the library's LLM client, not the app's conversation surface

`chat.rs` lives in `crates/epik`. The framing matters: this is
infrastructure that later also powers non-interactive uses (run
summaries, commit-message drafting) and tests, independent of whatever
the window's main pane eventually hosts. The same trinity as the
implementation side:

- **`ChatModel`** — a trait: transcript in, one streamed assistant reply
  out, blocking, deltas to a `Log<ChatEvent>` sink, stop token honored.
- **`ChatEvent`** — `Delta { text }`, `TurnFinished { usage }` (cost
  telemetry when the provider reports it), `Failed { error }`.
- **`Conversation`** — owns the canonical transcript (system prompt plus
  messages) and a `ChatModel`; `send` appends the user turn, streams the
  assistant turn, finalizes it. The frontend transcript is a *view*
  folded from events; that duplication is deliberate — core owns truth,
  UI owns a rendering of it.

The wire protocol is stateless (the full transcript is resent each turn),
so the transcript could technically live in the frontend. It lives in the
library so the daemon and every future client get "what a conversation
is" for free, rather than each reimplementing it.

**Model-agnosticism comes from the wire protocol, not a framework
crate.** The zeroth `ChatModel` is `Scripted` (deterministic, no
network — the unit-test model). The first real one is `OpenAiCompatible`:
a single hand-rolled HTTP + SSE client for the OpenAI chat-completions
format, configured by `{base_url, model}`. That one client covers OpenAI,
Ollama (local and free), Groq, Gemini, OpenRouter, and Anthropic's
compatibility endpoint. Multi-provider framework crates (`genai`, `rig`)
were considered and rejected: the needed surface is small, the trait is
Epik's own seam, and provider quirks are ours to absorb rather than a
pre-1.0 dependency's to reinterpret. A native Anthropic Messages client
remains possible later, behind the same trait.

A known wrinkle, flagged not solved: the eventual Claude Code adapter
should implement both `ChatModel` and the run-event side — one adapter,
two vocabularies. But `ChatModel` assumes the caller owns history, while
a Claude Code session owns its own. The adapter reconciles by sending
only the newest user turn; if that pinches, the answer is a narrower
trait for session-owning models, not a contortion of `ChatModel`.

### IPC

One command per intent (`send_message`, `set_api_key`), one event
channel. Every emitted `ChatEvent` is wrapped at the IPC layer in an
envelope carrying a conversation id. The host's registry assigns ids; a
`Conversation` does not know its own; the v0 UI ignores the field because
only one conversation exists. This one field is the whole architectural
cost of keeping multi-conversation UX open — single-main-chat is a UX
decision, not an architectural one, and retrofitting an envelope later is
the expensive version.

One in-flight turn per conversation, enforced by the host's mutex; the UI
disables the input while streaming.

### Configuration and secrets

Config types live in the library; the file lives in `~/.epik/`: provider
entries (`base_url`, `model`), the active provider, the system prompt
(default: the minimal Epik persona line). No settings UI — edit and
restart; the status bar names the active model.

**Secrets are pushed to official holders; Epik holds references, never
values.** Chat keys live in the OS keyring (`keyring` crate — Keychain /
Credential Manager / secret service) under service `Epik`, account =
provider name; the config file never contains a key. CI keys, when they
come, live in GitHub Actions secrets. Anthropic auth stays inside the
`claude` CLI login; GitHub auth inside `gh`. Each new secret kind gets a
case-by-case placement decision, and a minimal `KeyStore` trait (with an
in-memory implementation so tests never open a keychain) is the seam that
keeps those decisions independent. Resolution order: environment override
(`EPIK_API_KEY`) → keyring → absent, where absent renders a
paste-your-key card in the chat pane and `set_api_key` stores it.

### Tests and CI

| Tier | Model | Runs |
| --- | --- | --- |
| Unit | `Scripted` | Always: conversation logic, the SSE parser against captured payloads, the frontend fold. |
| Live | Ollama, sub-1B model | Locally: when a server is listening, skipped otherwise. In CI: always. |
| Paid | Real provider | Only when explicitly enabled by environment variable. |

The first real API test forces CI, and the secrets stance decides its
shape: **CI runs a local model in the job** — Ollama on the CPU runner,
model blob in `actions/cache`, no key existing anywhere. Tests assert
protocol properties (stream started, deltas arrived, turn finalized,
transcript grew), never answer quality, so a tiny model is adequate. The
laptop rule "skip when nothing is listening" must not reach CI, where a
silently skipped tier looks green forever: `EPIK_REQUIRE_LIVE=1` makes a
missing server a failure there.

## Consequences

- The current `src/` moves to `crates/epik`; `epik-app` and `epik-ui` are
  new; `epik-worker` stays as the daemon seam and headless harness.
- `logging.rs` splits into `event.rs` (vocabularies) and `logging.rs`
  (sinks); `Log` becomes `Log<E>`.
- The run cockpit, when it comes, inherits everything: the window, the
  envelope (a second vocabulary on the same channel), the fold
  discipline, the status bar, and the config/secret machinery.
- Done means: launch → status bar names the model → "Hi" → the reply
  streams as it generates → the theme toggle flips the whole window
  between light and dark → kill the network mid-turn and `Failed`
  renders as an error card, not a hang → point config at Ollama → same
  conversation, zero code changes, zero dollars.
- Deferred without prejudice: the Stop button (the token is already in
  every signature), multiple conversations, chat alongside a run, a
  settings UI, a native Anthropic client, a Desktop-facing MCP host.

## Note

The spike's deepest lesson carries over unchanged: the event types shared
with the frontend, the fold tested on the host, the unknown-tolerant
parser discipline. What this ADR supersedes in the spike is only its
orientation — chat as *the* object. Here chat is the first tenant of a
window whose landlord is the run.
