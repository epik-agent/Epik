# ADR: No clone necessary

- **Status:** Accepted
- **Date:** 2026-07-28
- **Source:** Design conversation (Cowork), July 2026

## Context

The question arose as a personal observation: the designer keeps a local
clone because browsing source in an IDE is pleasant — but nothing in Epik's
operating loop seems to require one. Should "no clone necessary" be the
official story?

The architecture already answers yes, silently. GitHub state is Epik's
interface (see the companion ADR, *Presence, not a connector*). The engine
builds in GitHub Actions runners on throwaway checkouts. Chat surfaces read
through the GitHub connector and write through the API (`gh_raw`). Launching
a feature, monitoring a build, reviewing a PR, cascading to dependent
issues — none of it touches the user's disk.

## Decision

Make it official: **the clone is a view, not a requirement.** Epik needs
GitHub, not your hard drive. A local checkout is a personal preference —
kept for IDE browsing the way one keeps a printed copy of a contract because
it is nicer to read in a chair — while the authoritative object lives
elsewhere. The story is the practical corollary of "any GitHub client is
already an Epik client."

The story is two-tier, and stays honest by saying so:

1. **Operating Epik** — designing, launching, monitoring, reviewing, from
   any Claude surface — requires no clone, ever.
2. **Joining a project as a terminal builder** is the one role where cloning
   matters, and there it is the *enrollment ritual*: clone + trust triggers
   the checked-in `.claude/settings.json` to self-install the plugin.

## Options Considered

### Option A: Clone-optional, officially (accepted)

| Dimension | Assessment |
|-----------|------------|
| Complexity | Low — mostly documentation; one skill change (init) |
| Cost | None ongoing |
| Fit with doctrine | Direct corollary of GitHub-state-as-interface |

**Pros:** truthful to the architecture; lowers the floor for new users
(operate Epik from chat with nothing installed but the plugin or MCP
prompts); sharpens the product story ("You say it. We make it." — from
anywhere).

**Cons:** requires closing the two leaks below so the story isn't aspirational.

### Option B: Clone assumed (status quo, implicit)

**Pros:** matches how the designer actually works day-to-day.
**Cons:** encodes a personal preference as a requirement; understates what
the architecture already delivers; makes the terminal the implied center of
gravity when the engine is Actions.

## Known Leaks

Two places the current implementation contradicts the official story:

1. **`epik:init` is directory-first.** It turns the *current directory* into
   an Epik project and then optionally creates the repo. The story implies a
   repo-first mode: scaffold `.claude/settings.json`, the CLAUDE.md stub,
   `loop.md`, and `docs/design-history/` straight through the API; cloning
   becomes optional and subsequent.
2. **Enrollment is clone-shaped.** Collaborator onboarding *is* clone +
   trust. This is not a contradiction once the story is stated as two-tier
   (operating vs. terminal building), but docs must not say "no clone, full
   stop."

## Consequences

- The README can state the principle in one line: *Epik needs GitHub, not
  your hard drive.* (Not Theory and Practice — that document is reserved for
  high-level design principles in an academic-paper/textbook register, not
  product story or process guidance.)
- A new issue: repo-first `epik:init` (scaffold via API, clone optional).
- Terminal-builder docs keep the clone+trust ritual, framed as enrollment
  for that role rather than a general prerequisite.
- The designer keeps his clone, guilt-free, as a view.

## Action Items

1. [x] ~~File the repo-first `epik:init` issue.~~ Superseded same day by
       [ADR: `epik:init` is idempotent convergence](2026-07-28-init-is-idempotent-convergence.md).
2. [ ] Add the principle to the README summoning/story section.
3. [ ] Reflect the two-tier framing wherever onboarding is documented.
