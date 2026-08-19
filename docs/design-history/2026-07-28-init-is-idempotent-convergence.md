# ADR: `epik:init` is idempotent convergence; the doctor retires

- **Status:** Accepted
- **Date:** 2026-07-28
- **Source:** Design conversation (Cowork), July 2026

## Context

Two overlapping skills shipped within a day of each other: `epik:init`
(turn the *current directory* into an Epik project, optionally creating the
repo) and `epik:doctor` (detect → offer → fix → report on an existing
setup). They are the same capability at different points on one axis — a
greenfield project is a maximally broken setup — and keeping both means
maintaining two descriptions of what a correct Epik project looks like,
which will drift.

Separately, the companion ADR *No clone necessary* flagged init's
directory-first orientation as a leak in the "Epik needs GitHub, not your
hard drive" story. And the doctor metaphor, while it named a good behavior,
was carrying no load of its own.

## Decision

Combine both into a single skill, **`/epik:init`**, whose contract is
**idempotent convergence**: converge this project to the correct Epik
shape, safe to run any number of times (the `git init` / Terraform-apply
contract). What it does depends on what it finds:

- **No repo** → create it, repo-first, entirely via the API: settings,
  CLAUDE.md stub, `loop.md`, and the `docs/design-history/` scaffold as the
  initial commit; labels, branch protection, and project board from
  `~/.epik` defaults. No clone involved — the repo it produces is
  self-installing on clone+trust, so cloning remains the terminal-builder
  enrollment ritual, exactly per the two-tier story.
- **Existing repo, not an Epik project** → offer conversion.
- **Epik project with drift** (broken auth, missing secret, stale
  workflow) → offer the fixes.
- **Healthy project** → say so and exit.

The doctor's *behavior* is entirely retained — detect → offer → fix →
report is how convergence on anything existing must run, and the offer step
is load-bearing: init is never destructive; it proposes deltas and the user
approves. Only the metaphor is retired.

**The canonical spec of a correct Epik project lives in the init skill** —
the former doctor checklist. Creation is "apply the whole spec to a fresh
repo"; repair is "apply the diff." One home, so nothing can drift against
it.

**Init stays explicitly invoked** (`disable-model-invocation`), for two
reasons beyond doctrine:

1. Typing `/epik:init` is the convergence gate applied to projects — the
   deliberate founding act that says "this is now an Epik project."
   Decision friction, preserved; the offer dialogue inside guards the
   individual changes.
2. A bootstrap asymmetry: init is the one skill that must be enabled
   *before* any Epik project exists, so it rides in the user-wide install
   and cannot rely on per-project enablement to keep it out of unrelated
   conversations. Explicit invocation is that containment.

The user types it rarely — the cold open ("new project, I know what I
want"), the hand-off (a summoned Epik notices the room is bare and points
at it), the occasional convergence check. Rarity is fine: its value is not
frequency but being the name of the founding ritual and the home of the
spec. A summoned Epik's offer ends with "run `/epik:init`"; the typed
command is the consent.

## Consequences

- Fold `doctor.md` into the init skill; delete `plugin/skills/doctor/`;
  reword the persona's hand-off sentence to offer init; rename the
  `setup-epik` MCP prompt (`init-epik`, or keep `setup-epik` as a friendlier
  alias on plugin-less surfaces); re-run the resource sync so vendored
  copies in `mcp/` follow; update the README summoning table.
- Supersedes the "file a repo-first `epik:init` issue" action item in
  *No clone necessary* — this decision is that issue, grown up.
- `~/.epik` defaults remain coherent from chat surfaces because EpikMCP
  runs on the user's machine even when the conversation doesn't.

## Note

The consolidation repeats the move recorded in the single-MCP ADR: a
boundary (init/doctor) drawn before practice tested it, found premature by
working it, collapsed while preserving the seam as internal structure —
creation and repair are branches of one spec, not separate deployables.
