# ADR: Builds come home — coding agents under `epik-worker`, GitHub in the library, tools behind one registry

- **Status:** Accepted
- **Date:** 2026-08-05
- **Source:** Design conversation (Cowork), August 2026

## Context

The chatbot walking skeleton (#82, PR #91) landed, and first ad hoc use
produced two lists. The cosmetic one became feature #101 — five
presentation-only fixes, deliberately chosen as the dogfood target for
what this ADR designs. The substantive one exposed the real gap: the app
has no tools, no GitHub access, and no way to set coding agents to work
on a tree of issues.

Meanwhile the plugin-era pipeline is in a broken transitional state. The
greenfield merge removed `mcp/`, `plugin/`, and `epik-build.yml` from
`main`; installed copies of EpikMCP run only from a uvx cache pointing at
a deleted subdirectory, and `feature_launch` has nothing to dispatch. The
question is not how to restore that pipeline but what replaces it.

The frame: **what does the Epik app need in order to build #101
itself?** Two things. Read/write GitHub access, including the GraphQL
mutations for sub-issue and blocked-by relationships. And the ability to
set coding agents on an issue tree, concurrently where the graph allows.

Standing constraints from the walking-skeleton ADR all hold: the library
is synchronous and wasm-clean; anything the app can do must be achievable
through `epik` library calls alone; secrets are pushed to official
holders; UI state is a pure fold over events.

## Decision

### GitHub is a library client — not an MCP, not `gh`

`github.rs` lives in `crates/epik` beside `chat.rs`: a hand-rolled
client, REST for the ordinary verbs and GraphQL (a POST to `/graphql`)
for the relationship mutations that never got REST endpoints. The typed
surface covers exactly Epik's verbs, nothing more. `octocrab` was
considered and rejected on the `OpenAiCompatible` precedent: it drags
async/tokio against a synchronous library and misses the GraphQL
mutations anyway. Existing GitHub MCP servers were surveyed and
rejected: sprawling, coarse-grained, and thin exactly where the
sub-issue and blocked-by mutations live. `gh` disappears from the app.

It is a module, not a crate. No consumer of "GitHub minus Epik" exists:
every door (below) depends on `epik`, never on a GitHub crate directly,
because the doors speak Epik's verbs — feature, issue graph, launch —
not raw GitHub. If a second consumer ever appears, extraction from the
workspace is mechanical.

**This revises one line of the walking-skeleton ADR** ("GitHub auth
inside `gh`"). The library's client resolves its token on the
`keystore.rs` rails: `EPIK_GITHUB_TOKEN` → keyring → absent, with the
`Found`/`Absent`/`Unreachable` refinement carried over. A fine-grained
PAT pasted once suffices now; a GitHub App with the OAuth device flow is
the later, monetization-shaped seam, left open but not built. Wrapped
coding agents keep their own auth (`claude` login, `gh` login) —
transitional, and their problem.

**Repo secrets are written by moving, never by copying.** Provisioning a
GitHub Actions secret is one operation: the sealed-box PUT, then deletion
of the keychain entry. GitHub's API is write-only for secrets, which
makes the semantics honest — the local copy really was the last readable
one. A secret needed both locally and remotely is an explicit, loud copy
the user asks for, never a side effect. The demand side tends toward
zero: CI is secretless by design and builds no longer run in Actions.

### Tools are one registry; every consumer is a door

The library defines a tool registry — name, description, schema,
handler — populated once and surfaced everywhere:

- The in-app chat loop reads it natively. The GitHub verbs are the first
  native tools; the chat UI is their primary consumer.
- A future `epik-mcp` stdio binary exposes the same registry to Claude
  Desktop: a sibling host to `epik-app`, thin by the invariant.
- The registry interface is transport-agnostic: a tool may be backed by
  a local function or by an MCP *client* call. Linear and its like
  arrive later as MCP-provided tools with zero shim-writing. Native
  where we must (GitHub), MCP where we can.

Parallelism-as-a-service is reserved, not built: a `delegate(task)`
registry tool backed by the trait below would let any tool-calling agent
fan work out through the harness — visible, budgeted, supervised. The
issue graph already encodes work-splitting judgment, so nothing is
needed now.

### Coding agents: vocabulary first, one trait, one verb

The `ChatModel` decomposition repeats for implementation. In
`epik::agent`:

```rust
pub struct Task {
    pub kind: TaskKind,          // Implement — later Review, FixCi, ...
    pub prompt: String,          // rendered by the harness from a template
    pub worktree: PathBuf,
    pub env: Vec<(String, String)>, // credentials injected, never discovered
    pub budget: Budget,          // cost ceiling + stall window
}

pub enum AgentEvent {
    Started { agent: String, version: String },
    Progress(String),            // narrative, for the console
    Usage(Usage),                // the budget watches this
    Detail(serde_json::Value),   // opaque, agent-flavored, rendered best-effort
    Finished(Stop),
}

pub enum Stop {
    Completed,                   // the agent believes it finished
    Blocked { report: String },  // the dignified exit
    Spent,                       // budget exhausted
    Stalled,                     // silent past the stall window
    Canceled,                    // stop token honored / process killed
    Died { error: String },
}

pub trait CodingAgent {
    fn run(&self, task: &Task, sink: &mut dyn FnMut(AgentEvent))
        -> Result<Stop, AgentError>;
}
```

Design properties, each deliberate:

- **Steps are data, not methods.** A new agent implements one verb and
  inherits every `TaskKind`. A wrapper may special-case a kind
  internally (an agent with a native review mode); the harness never
  sees the difference.
- **There is no `Ask` variant.** Autonomy is structural: a wrapper has
  nowhere to put a question except `Blocked { report }`. Prompts still
  say "don't stop to ask; when truly blocked, fail loudly with a
  report" — the belt to the vocabulary's suspenders.
- **`Stop` is not "done."** It says only how the process ended. Done is
  the harness's judgment, made out-of-band through `github.rs`: the PR
  exists, CI is green. Self-reported success is not in the vocabulary.
- **`Usage` is one vocabulary; tokens are the base unit; money is
  reported, never synthesized.** The same type serves `ChatEvent` and
  `AgentEvent`: integer token counts (input, output, cache when
  distinguished) always present; `cost: Option<Money>` (fixed-point, not
  `f64`) populated only when the agent states it — Claude Code does,
  Ollama honestly doesn't, and a maintained price table in Epik is a lie
  waiting to go stale. Events carry *cumulative* totals, monotone
  nondecreasing (a conformance assertion), with the `Usage` at
  `Finished` authoritative. `Budget` mirrors the denominations —
  `max_tokens`, `max_cost`, stall window — enforcing whichever are set;
  a budget denominated in a unit the configured agent never reports is a
  launch-time config error, not a silent no-op ceiling.
- **Synchronous, like everything else.** One thread per running agent,
  blocked in `run`, feeding a channel — the Tauri host's own pattern.
  Wrappers may keep private threads (a reader feeding `recv_timeout` is
  how a stall window works over a blocking pipe); async never reaches a
  signature. MCP transports (rmcp) bring tokio into host binaries only,
  adapted at the edge.

Two implementations from day one: `Scripted` (the unit-test agent — the
scheduler, supervision, and console are tested with no key and no
Claude) and `ClaudeCode` (`std::process::Command`, cwd the worktree,
`claude -p --output-format stream-json --verbose`, permissions
pre-granted, JSON lines folded into `AgentEvent`s — dependency-free
beyond `serde_json`). Governance necessarily lives inside each wrapper
(a black box mid-`run` cannot be killed from outside), so the trait
ships with a **conformance test suite**: hostile scripts asserting an
implementation stops when spent, stalls out, and never emits after
`Finished`. The contract is enforced by CI, not review vigilance.

Claude Code's internal subagents stay out of the contract — an
implementation capability, surfaced through `Detail`, costs rolled into
`Usage`. The wrapper's `allowed_tools` is the dial: work-splitting
judgment belongs to the issue graph, not the agent.

### Builds leave GitHub Actions; `epik-worker` is the venue

`epik-build.yml` stays dead. Agents run as local processes under
`epik-worker` — initially `epik/src/bin/epik-worker.rs`, a thin main
over the library (read config, resolve keys, schedule, spawn, pump
events), promoted to its own crate only when a dependency the wasm build
must not carry forces it. CI's role is unchanged and singular: the
neutral arbiter of green on PRs.

Scheduling is a pure fold over the blocking graph, host-tested like
everything else. Edges encode *implementation* dependencies only. There
is no scheduler contention story: each implementer's definition of done
includes "resolve any merge conflicts and end green," the same contract
human developers work under.

**Wide now, tapering by evidence.** v1 keeps the existing `feature` and
`issue` prompts essentially as-is — they move from plugin skills to
templates the harness renders (issue, branches, conventions in; one
prompt out). The agent owns the whole loop, prompt-instructed. Each
taper moves a "what happens next" decision from agent to harness; the
smaller prompt is the residue of the move, not the move itself. The
carve criterion, learned empirically: a seam is carvable when the
harness can assemble the input deterministically (diff, log, issue text)
and verify the output without trusting the agent's narrative. Purely
mechanical actions (branch, push, open the review PR — the generalized
#70 lesson) migrate into deterministic code as soon as possible; the
tool array shrinks in sympathy, so least privilege falls out of the same
process. What seems like a natural cleavage point may in fact be better
left to the agent — that is an empirical question the build logs answer,
and the reason to start wide.

The trust posture changes and is recorded as a decision, not an
accident: Actions gave agents a disposable sandbox; `epik-worker` runs
them as local processes with the user's credentials, confined to
worktrees by convention. Container confinement is deferred without
prejudice.

### The filesystem is plumbing; git is one mechanism

**Epik owns a private clone; the user's checkout is never touched;
GitHub is the only rendezvous.** Per repo, a bare clone under
`~/.epik/repos/<owner>/<name>/`, created lazily, fetched at the start of
each `FeatureRun`. Deltas occur only in worktrees hung off it
(`~/.epik/work/.../issue-<n>/`) — the `Task.worktree` the harness hands
the agent. Worktrees share one object store; each gets its own `target/`
(a shared `CARGO_TARGET_DIR` serializes concurrent builds on Cargo's
lock; `sccache` is the deferred answer if disk stings). Integration
never happens locally: branches are pushed, and all merging is
GitHub-side through `github.rs`; even conflict resolution is a rebase in
the implementer's own worktree, settled at the remote. The invariant
this buys: **everything under `~/.epik/repos` and `~/.epik/work` is a
cache of GitHub — deleting it loses nothing but time** — and Epik can
manage a repo the user has never personally cloned. Worktrees are
deleted on success, retained on `Blocked`/`Failed` for forensics until
dismissed.

Git verbs go through one mechanism: a thin `epik::git` module — typed
surface, structured errors, stringliness confined — implemented by
spawning the `git` binary. This is not the `gh` contradiction: `gh`
wrapped an HTTP API we speak directly, while `git` has no simpler
protocol beneath it; the in-process alternatives (`git2`, `gix`) are
weakest exactly at worktrees and rebase, and every agent already
requires the git binary anyway, so harness and agent share one git.
Bundling git is rejected (GPLv2 obligations, weight, near-tautological
audience); if `gix` matures, it swaps in behind the seam. When the
harness starts pushing (post-taper), it authenticates with the
keystore's token via an askpass shim or in-memory header — never a
token in a remote URL or gitconfig.

**Stated design goal: from the user's point of view, Epik has no
filesystem.** The user's objects are issues, features, runs, diffs,
PRs; the disk is to Epik what its cache is to a browser. Consequences
accepted now: the console renders paths repo-relative (agent narrative
will otherwise leak `~/.epik/work/...` in every cargo error); failure
forensics happen in-app, with "open in editor" as an explicit escape
hatch rather than ambient path-awareness; cache size and clearing are
settings-surface concerns, never a Finder errand; and "if it isn't
pushed, Epik can't see it" is a feature statement, not a limitation.
Acceptance for the goal: every user story completes without a Finder
window. Whether it survives contact with real forensics is empirical;
it is a goal, not yet an invariant.

### Refusals are vocabulary, not log prose

"What's wrong" is data. The existing shapes already half-enforce this —
`Stop`, `Blocked { report }`, the keystore's
`Found`/`Absent`/`Unreachable` — and the rule is now general: failures
and refusals are typed vocabulary rendered by doors, never prose
scraped from a log.

First consequence: **`epik-worker` preflights a config-derived manifest
before any run** — git present and above a version floor, the
configured agent's binary, the GitHub token, provider reachability —
and refuses with a structured `CapabilityStatus` naming the capability
and the locations searched, before a worktree exists. The manifest has
layers with different owners: Epik's own (git, and nothing else),
per-agent (the wrapper declares its binaries; `gh` retires with the
PR-ceremony taper), per-repo (the target's toolchain is the user's
responsibility, with `Blocked` as the honest failure), per-provider
(reachability, not spawning). Every external binary is resolvable
explicitly in config with PATH as fallback, not mechanism — launchd's
PATH is not a login shell's — and on macOS the git check consults
`xcode-select -p` rather than trusting Apple's dialog-popping shim.
Boot-integrity failures (unparseable config, unwritable `~/.epik`) are
the only fatal ones; capability absences degrade and report.

### The plugin era ends deliberately

The Python EpikMCP, the plugin skills, and the Actions dispatch are
transitional scaffolding with a planned death, not artifacts needing a
home. `feature_launch`'s Actions dispatch retires with the workflow it
dispatched. `/epik:feature` and `/loop` are replaced over time by the
manager console — the chat window driving the registry's verbs. Claude
Desktop as an optional chat UI remains possible at the cost of the thin
`epik-mcp` shim, with one rule: the worker pool has exactly one owner,
and the MCP door forwards to it — never a second supervisor.

## Consequences

- `crates/epik` gains `github.rs`, `agent.rs` (vocabulary, trait,
  `Scripted`, `ClaudeCode`), the tool registry, and
  `src/bin/epik-worker.rs`. The keystore gains a second entry kind.
- **Dogfood: #101 is the first feature built by the new pipeline**,
  launched from the `epik-worker` binary; the window takes over
  launching once the chat loop consumes the registry. Its five unblocked
  sub-issues all touch the same small frontend — a deliberate stress
  test of implementer-owned conflict resolution. #94's acceptance (Dock
  icon, About window) is macOS-only and is carried by the human review
  of the feature's review PR, not by the harness.
- Issues #98/#99/#100 (closed, not planned) are answered here: the tool
  loop's transport question resolves as native-plus-MCP-client behind
  one registry; the filesystem provider arrives as registry work; the
  EpikMCP question resolves as planned death. #97 (rich rendering)
  remains open and undesigned, deliberately.
- The worker feature's acceptance includes the refusal story: run
  `epik-worker` with git absent, or no token, or a dead provider, and
  the result is a structured refusal naming the capability and the
  locations searched — correct exit code, no backtrace, no worktree
  created.
- Deferred without prejudice: `TaskKind`s beyond `Implement`; the
  GitHub App device flow; the `delegate` tool; container confinement;
  whether the cached Python EpikMCP gets a repaired install pointer or
  simply rides its cache until `epik-mcp` exists; the daemon host's
  degraded-boot and health-endpoint story, which builds on
  `CapabilityStatus` but is its own ADR when the daemon arrives; the
  settings UI that the filesystem-opacity goal quietly promotes from
  nicety to obligation (today "edit `~/.epik/config` and restart" is
  itself filesystem access).

## Note

Every capability has entered Epik the same way twice now: a vocabulary,
a trait, a scripted implementation, and thin hosts that add only what
their transport demands. `ChatModel` is to chat what `CodingAgent` is to
implementation. The other recurring motif is #70's lesson generalized:
a guarantee that lives in a prompt holds for one agent on a good day;
guarantees worth having migrate into machinery — which is what the
taper is.
