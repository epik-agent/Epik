# ADR-0002: Consolidate Epik's MCP into a single component

- **Status:** Accepted. Supersedes ADR-0001 Decision 2 (three loosely-coupled repos).
- **Date:** 2026-06-27
- **Source:** Design conversation (CoWork), June 2026

## Context

ADR-0001 proposed three repos: `epik-gh` (GitHub mechanism), a separate Claude-API MCP (`epik-build`), and the plugin. Working the boundary — what to name the GitHub server once it's named by responsibility, whether the two MCPs should merge because a full "monitoring" view spans both — turned into an extended philosophical discussion.

That difficulty was itself the signal. We were drawing component boundaries across code that doesn't exist yet, justified by fluent argument rather than anything built. In Theory/Practice terms, it was manager-mode architecture on an unconverged theory — and the ease of generating ever-finer justifications for the split was the fluency trap, not evidence the split was right.

## Decision

Consolidate the GitHub and Anthropic functionality into a **single MCP component**, `EpikMCP` (repo `wpm/EpikMCP`, renamed from `epik-gh`). The plugin declares one MCP server.

Internally, keep the two concerns in **separate modules**:

- a **plan** module (GitHub): author the feature's issue graph and read its status — issues, relationships, projects, labels, repo, read-only PR/runs, plus `gh_raw` and `feature_status`;
- a **build** module (Anthropic): launch a feature build and monitor the session.

This is ordinary cohesion hygiene, not pre-splitting. It keeps a future extraction a few-hour refactor rather than a rewrite — *provided* the two modules don't entangle their auth and state.

The responsibility distinction from the merge discussion still holds and is still correct — **system of record for the work** versus **control plane for the workers** — but it is expressed as an internal module seam, not as separate deployables. Naming-by-responsibility is deferred: with one component, "Epik's MCP" is a true and sufficient name.

## Triggers to revisit (split out a second component only when one fires)

- A real need to reuse the GitHub half standalone, outside Epik.
- The research-preview Anthropic APIs (routines / Managed Agents) churning hard enough to destabilize the stable GitHub side.
- The single repo's tooling or CI starting to fight itself (Python deps, test isolation, release cadence).

Until one of these bites: YAGNI.

## Consequences

- One repo, one package, one `.mcp.json` entry, one issue set. Simpler to build and iterate now.
- The earlier `epik-build` repo is not created; its scope (launch + monitoring) becomes the build module of EpikMCP.
- The launch/monitor path has an unresolved coherence question — routines `/fire` is launch-only with no monitoring API, while the Managed Agents API offers full monitoring but is a separate, metered session system. Deferred to a spike inside the build-module issue rather than decided here.
- Follow-ups: the plugin's `.mcp.json` and README, and references in ADR-0001, still say `epik-gh` and need updating to `epik-mcp` / `EpikMCP`.

## Note

This ADR is an instance of the method it records: the boundary was found premature by trying to specify it, not by reasoning further. The cheap move was to collapse and let Practice redraw the seam later, if it ever needs redrawing.
