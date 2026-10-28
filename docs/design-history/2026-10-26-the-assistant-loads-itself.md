# The assistant loads itself

- Status: Accepted
- Date: 2026-10-26

## Context

Epik should know it is Epik as soon as a conversation starts. The planning
prompt from the last Rust release did this inside Epik's own app; the plugin
has had nothing in its place, so a conversation knows it is Epik only if the
user says so.

Two other ways in are rejected for now:

- **A summoning command.** The user would have to remember to type it, and
  a conversation that started without it would not be Epik.
- **A separate Epik app.** It may return with the deferred host or monitor
  (see [local-builds](2026-10-26-local-builds.md)).

## Decision

Epik's MCP server sends short `instructions` that say what Epik is and tell
the session to read Epik's `assistant` skill. The skill holds the planning
prompt, updated from milestones and features to epics, and the `rules.md`
that binds every change to the code.

### Why the exception to `disable-model-invocation`

The `epic` and `issue` skills set `disable-model-invocation: true`, so the
only way into them is the slash command. The `assistant` skill does not. A
skill that loads at the wrong moment costs almost nothing when it only shapes
how Claude plans and writes. It costs a lot when it acts: `epic` and `issue`
create branches, start builds and merge.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| Server `instructions` plus a self-loading skill | Chosen | Reaches every conversation with the plugin enabled; no command to remember. |
| A summoning command | Rejected | Epik is only Epik when the user remembers to ask. |
| A separate Epik app | Rejected for now | May return with the deferred host or monitor. |

## Consequences

- **The instructions reach every conversation** that has the plugin enabled,
  including ones that have nothing to do with Epik. So they stay a few
  sentences long, and the substance lives in the skill.
