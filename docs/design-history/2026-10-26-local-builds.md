# Builds run locally, and an epic is its sub-issues

- Status: Accepted
- Date: 2026-10-26
- Supersedes: §4 ("Where each kind of work runs") and §5 ("Determinism
  grows as scripts beside tools") of
  [one-plugin-standard-github](2026-10-24-one-plugin-standard-github.md)

## Context

The [one-plugin](2026-10-24-one-plugin-standard-github.md) entry made cloud
builders the default. Two goals pull on that choice:

1. **The user never types a build command.** They tell the Epik persona to
   build an issue or an epic. The persona starts the build with a
   deterministic tool and never builds inside its own conversation. The
   `epic` and `issue` skills keep `disable-model-invocation: true`, so the
   only way into them is a session whose first message is the slash command.
2. **Builds run in the cloud,** so they continue with the laptop closed.

### Findings (2026-10-26: docs, plus one local test)

| Question | Finding |
|---|---|
| Can a program start a cloud session? | **Not without a terminal.** `claude --cloud "<task>"` creates one interactively. `claude -p` rejects `--cloud` with a task description; with `-p`, `--cloud` only queues a message into an existing session. |
| Do other routes start cloud sessions? | **Routines** fire over HTTP, but each needs a routine and a bearer token created by hand on the web, which Epik would hold. **Claude Code Projects** start cloud threads from a coordinating conversation, but are a Pro/Max beta and launch at the model's discretion. |
| Can a program start a local session? | **Yes.** `claude -p "/<skill> …"` expands a user-invoked skill and runs headless. `--output-format stream-json` streams the run. |
| Does a local headless run start a plugin's MCP servers? | **Yes.** Only cloud sessions switch plugin MCP servers off. |
| Does `--plugin-dir` collide with an installed copy of the same plugin? | **Both load** (`epik@epik` and `epik@inline`; tested with `claude plugin list`). `--settings '{"enabledPlugins":{"epik@epik":false}}'` turns the installed copy off for that run. |

Cloud and Epik's MCP server can't be had together. Goal 1 needs the server;
goal 2 needs the cloud.

## Decision

### 1. Builds run locally, for now

Epik's server starts each build on the user's machine: a fresh clone under
`~/.epik/builds/`, a detached headless `claude -p` running `/epik:epic` or
`/epik:issue`, and a log under `~/.epik/runs/`. The build loads the plugin
copy the server itself runs from, and runs in auto mode with no permission
prompts.

The user can no longer close the laptop while a build runs. That is the
price, and it matches how Epik is used today.

A user may still start a cloud session by hand and type the slash command.
Epik does nothing to support or prevent that.

### 2. Builders use Epik's tools directly

A local build has Epik's server, so the `epic` skill gets its dependency
order from `epic_status` rather than from the model. The command-line script
planned in §5 of the one-plugin entry is not needed.

### 3. An epic is its sub-issues

An epic is a graph of issues. Its members are the epic issue's sub-issues,
and its edges are their blocked-by relationships. Issues merely linked in an
epic's text are not members; the persona turns them into sub-issues before a
build. So "is this an epic?" has a deterministic answer: it has sub-issues.
The `epic` label stays, for people.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| Cloud builders started by a routine's API trigger | Rejected | A routine and token per repository, set up by hand, with the token held by Epik. Breaks standard install and no custody of secrets. |
| Claude Code Projects as the persona's surface | Rejected for now | Beta on some plans only, and the model, not a tool, decides when a build starts. |
| `claude --cloud` under a pseudo-terminal | Rejected | Depends on undocumented interactive behavior. Non-standard. |
| A host or monitor application, run as a laptop daemon or remotely | Deferred | Duplicates what cloud sessions already provide. |
| Epics defined by sub-issues or by issues linked in the text | Rejected | Text links can't be told apart from mere references. |

## Consequences

- **The laptop stays awake during builds.** Revisit when Anthropic offers a
  documented way to start cloud sessions that keep plugin MCP servers, or at
  Epik 2.0, whichever comes first.
- **New prerequisite:** the Claude Code CLI, installed and signed in, on the
  machine that runs Epik's server. This applies to Desktop-only users too.
- **Build output lands under `~/.epik`,** the data source for a future
  monitor.
- **Plugin MCP servers off in cloud sessions no longer matters** to Epik's
  own builds.
