# ADR-0003: Feature builds run on Claude Code; Managed Agents rejected

- **Status:** Accepted
- **Date:** 2026-06-28
- **Source:** Design conversation (CoWork), June 2026

## Context

EpikMCP's build module needs to launch a feature build remotely, and we considered whether it should also monitor the session. The candidate for real session monitoring (status, events, streaming) was the Claude **Managed Agents** Platform API.

## Decision

Feature builds run on **Claude Code** (via the routines `/fire` API), not on Managed Agents.

- Managed Agents is a *different harness* — a generic agent loop (model + prompt + Bash/file/web/MCP/skills in a sandbox) that you assemble yourself. It is not Claude Code and does not provide Agent Teams, the `/feature` command, plugins, or the GitHub proxy — all of which the Epik build depends on. Running the build there would be a re-platforming, not a monitoring choice.
- A routines-`/fire`-launched session is a Claude Code session, **not** a Managed Agents session, so the Managed Agents API cannot observe it.
- Routines `/fire` itself exposes no programmatic run-status; CCotW monitoring is the web UI / session URL only.

Therefore the build module's only tool is `feature_launch` (launch + return the session URL). **Progress monitoring is `feature_status` (GitHub effects) plus the session URL** — the "GitHub is the database" principle. Managed Agents is considered and rejected.

## Consequences

- `feature_launch` is launch-only; there is no session-monitor tool.
- Progress visibility comes from the GitHub system of record, not the agent runtime.
- Revisit only if Anthropic exposes a programmatic status API for routines/CCotW sessions, or if abandoning the Claude Code harness (and Agent Teams) ever becomes worth it for some other reason.

## How we learned this

Documentation, not experiment. The Managed Agents overview and the "decoupling the brain from the hands" architecture make the harness distinction clear. The definitive ground-truth check — fire a routine, then try to GET its session through the Managed Agents session API — was judged an unnecessary formality. (This is the intended shape of investigation: resolve it in conversation + docs, record the decision here, then write the planned issue — never as a spike issue.)
