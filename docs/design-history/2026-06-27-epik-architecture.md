# ADR-0001: Epik architecture and workflow

- **Status:** Accepted. Decision 2 (three repos) superseded by [ADR-0002](2026-06-27-consolidate-single-mcp.md) — single-component consolidation.
- **Date:** 2026-06-27
- **Source:** Design conversation (CoWork), June 2026

## Context

Epik automates a manager-mode development workflow the author already runs by hand: have a long design conversation with an AI, converge on a plan, break it into GitHub issues, and let autonomous agents implement them while the human supervises. The unit of work is a **feature** — a unit of code implemented in one or more stories (issues). Epik is built around GitHub (non-negotiable); a project is typically a single GitHub repo checked out in the project directory.

The guiding philosophy is **Theory/Practice** (after Naur's *Programming as Theory Building*): the program is a formal model of an application domain, built only by putting theory into practice; the human holds the theory, and agents are supervised revival attempts. Epik's job is to keep the human resident in the thinking and let the mechanical layer recede — without ever letting invisible mechanism hide its consequences.

## Decisions

1. **Epik is a Claude Code plugin.** One install brings the workflow: it bundles commands, hooks, and MCP declarations. This also makes it remote-ready, because everything is repo- or marketplace-sourced — exactly what Claude Code on the web carries into a cloud session.

2. **Three repos, loosely coupled.**
   - `epik-gh` — GitHub *mechanism*. A curated MCP wrapping the `gh` CLI. Domain-agnostic, reusable, its own lifecycle.
   - A separate *Claude-API* MCP — for non-GitHub operations (Claude Code routines `/fire`, etc.). Mechanism for talking to Anthropic, not GitHub.
   - The Epik *plugin* — *policy*. The opinionated feature workflow (commands, hooks) plus its own single-entry marketplace. It *declares* the two MCPs; it does not contain them.

   Rationale: mechanism versus policy. A plugin depends on an MCP the way an app depends on a library — not by vendoring its source.

3. **Two surfaces, deliberately distinct.**
   - **CoWork** — design/theory and feature authoring; the human's primary surface. CoWork has no native GitHub, so `epik-gh` is the only way to write a feature's issue graph here.
   - **Claude Code on the web (CCotW)** — autonomous execution, with its own native GitHub access. Launched *from* CoWork via the routines `/fire` API, so the human never switches surfaces to start a build. CCotW is an observation/intervention surface, not an initiation one.
   - **Local IDE Claude Code** — *ad hoc* mode (hands-on, theory-building). Deliberately separate; the surface boundary is the conscious mode switch.

4. **`feature` and `issue` are commands, not skills.** Commands are deliberate, user-typed triggers — the right shape for the manager-mode handoff. `feature` orchestrates; `issue` implements a single issue. (Formerly named `epic`.)

5. **Branch naming.** The umbrella branch is the **feature branch**; each issue is implemented on its own **issue branch** whose PR targets the feature branch. (Renaming `epic`→`feature` forced this split, since the old scheme called per-issue branches "feature branches.")

6. **`epik-gh` scope: author the graph + read for status.** Keep issues (CRUD), relationships, projects, labels, repo, and read-only PR/run tools. Drop execution writes (PR create/merge, branch create/delete) — CCotW runs execution natively. A `gh_raw` passthrough covers the long tail so the trim loses nothing.

7. **The relationship layer is the keystone.** Feature→issue (sub-issues) and dependency ordering (blocked-by) are how a feature's issues are structured and parallelized. This is the newest, least-served part of GitHub's API and the part `epik-gh` must get right (current API: REST `addBlockedBy`/`removeBlockedBy`, GraphQL `subIssues`/`parent`).

8. **Status: GitHub is the database.** No custom dashboard. Use GitHub Projects and "ask CoWork what's going on." The only state GitHub can't see is "work started" — a local agent act with no GitHub footprint — which is the one thing worth a hook.

9. **Theory/Practice guides the machine's behavior.**
   - A **convergence gate** lives at the feature-launch tool — the point of no cheap return — asking whether the theory is mature enough to delegate. Not encouragement; a check that can refuse.
   - **Clerical vs decision seams:** dissolve clerical friction (the CoWork→CCotW switch, copy-paste markdown); preserve decision friction (the conscious mode switch, the gate).
   - **Forcing functions over instructions:** invariants the model can't be trusted to honor in prose (e.g. "never touch the default branch") become hooks, not command text.
   - **Invisible mechanism, visible consequence:** hide the *how*, never the *what you now own*.

## Consequences

- The human's surface shrinks to: live in CoWork, glance at GitHub/Projects, and drop into a CCotW session only to watch or to rescue a run that has hit the swamp. Everything else is plumbing configured once.
- The human is the only bridge between manager mode and ad hoc mode; theory built hands-on doesn't auto-flow into features. Accepted, because the human *is* the theory.
- Launching a feature build remotely is feasible via the Claude Code routines `/fire` API (per-routine token + dated beta header). It requires a saved "feature runner" routine, unrestricted branch pushes enabled on the repo, and `epik-gh` present as a claude.ai connector or a committed `.mcp.json`.
- `epik-gh`'s dependency tools are currently broken (a preview mutation renamed at GA); repairing them is prerequisite to feature orchestration and to status.

## Note

This is a Practice-phase document — an ADR as a bridge to code. The design conversation is the source; this captures the theory delta worth carrying forward.
