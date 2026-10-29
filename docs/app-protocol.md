# The Epik app's protocol

Other programs, chiefly Epik's MCP server, start builds and read runs by talking to the Epik app over a local socket. This is the contract between them. The app's side is `epik_core::ipc`.

## The socket

- On macOS and Linux, the Unix domain socket `~/.epik/epik.sock`. Its mode is `0600`, so only the user can open it.
- On Windows, the named pipe `epik`, that is `\\.\pipe\epik`. It refuses remote clients, and Windows's default
  security for a pipe lets only its creator, administrators and the system write to it.

There is no other authentication. If nothing is listening, the app isn't running. A socket file left by an app that
crashed is removed when the app next starts.

## Messages

A client connects, writes one request as a single line of JSON ending in `\n`, and reads one line of JSON in reply.
Then the connection is done; each request takes a new connection. Every message is an object tagged by its `"type"`.
Fields a request doesn't know are ignored.

Any request may be answered with an error:

```json
{"type": "error", "message": "couldn't build owner/repo#8: it is closed"}
```

A request that isn't JSON, has an unknown `type`, or lacks a field gets an error whose message starts with
`bad request: `.

## Runs and builds

A **run** is the work of finishing one epic, or one issue with no sub-issues. It is named by its epic issue,
`owner/name#number`, and that is how every request names it. A **build** is one attempt at a run. Building an epic again
after a failure or an interruption adds a build to its run, and the run's elapsed time and cost add up across its
builds. A run's record is `~/.epik/runs/<owner>/<name>/<number>/`, with a directory per build.

A run leaves when its epic issue closes: the app comments its totals on the issue and deletes its files.

### `build`

Starts building an epic or a single issue.

```json
{"type": "build", "repo": "owner/name", "number": 356, "branch": "daemon", "plugin_dir": "/path/to/epik/plugin"}
```

- `repo`: the repository, `owner/name`.
- `number`: the epic or issue. An issue with sub-issues is built as an epic.
- `branch` (optional): the branch the issues' pull requests merge into. By default `epic/<number>-<slug of title>` for
  an epic, and the repository's default branch for a single issue.
- `plugin_dir`: Epik's Claude Code plugin, which each agent loads.

The app replies once the build has started: the issue is read, the repository cloned, and `build_started` written to
the build's record. That can take as long as the clone. The build then goes on in the app. When the epic issue already
has a run, the build is added to it; while one of its builds is running, another is refused.

```json
{"type": "started", "run": "owner/name#356", "build_id": "20261028T212859Z-k3f9", "kind": "epic", "branch": "daemon", "build_dir": "/Users/me/.epik/runs/owner/name/356/20261028T212859Z-k3f9"}
```

- `build_id`: the build's id, unique within its run: the UTC time it started and 4 random characters.
- `kind`: `epic` or `issue`.
- `build_dir`: the build's record, holding `events.jsonl`, each issue's agent output, and `summary.json` once the
  build ends.

A pull request, a closed issue, or a failure before the build starts is an error, and leaves no record.

### `status`

A run's state.

```json
{"type": "status", "run": "owner/name#356"}
```

The reply is the run's state, folded from its builds, with `"type": "run"`:

```json
{
  "type": "run",
  "run": "owner/name#356",
  "repo": "owner/name",
  "number": 356,
  "title": "The Epik app runs builds",
  "kind": "epic",
  "branch": "daemon",
  "disposition": "building",
  "reason": null,
  "pull_request": null,
  "issues": [
    {
      "number": 357,
      "title": "Rust workspace and CI",
      "blocked_by": [],
      "status": "merged",
      "reason": null,
      "pull_request": 367,
      "merge_commit": "dd1288cc693368c025b69618edc4bc603c07d301",
      "duration_ms": 512000,
      "cost_usd": 1.12
    },
    {
      "number": 358,
      "title": "The record",
      "blocked_by": [357],
      "status": "running",
      "reason": null,
      "pull_request": null,
      "duration_ms": null,
      "cost_usd": null,
      "activity": {
        "last_action": "Running cargo test",
        "last_test": "23 passed, 1 failed",
        "last_event": "2026-10-28T21:38:57Z",
        "recent": ["Reading src/record.rs", "Editing src/record.rs", "Running cargo test"]
      }
    }
  ],
  "builds": [
    {
      "build_id": "20261028T212859Z-k3f9",
      "status": "running",
      "reason": null,
      "pull_request": null,
      "started_at": "2026-10-28T21:28:59Z",
      "ended_at": null,
      "elapsed_ms": 600000,
      "cost_usd": 1.12
    }
  ],
  "running_build": "20261028T212859Z-k3f9",
  "started_at": "2026-10-28T21:28:59Z",
  "latest_at": "2026-10-28T21:38:59Z",
  "elapsed_ms": 600000,
  "cost_usd": 1.12
}
```

- `disposition`: `building` while a build runs; `waiting_for_review` when every issue merged and the pull request into
  the default branch waits for the user; `needs_attention` when the latest build failed or was interrupted, or its
  pull request was closed without merging. `reason` says why a run needs attention.
- `pull_request`: the latest build's pull request into the default branch: the epic's, or a single issue's.
- `issues`: every issue of the run, including ones merged in earlier builds. Each one's `status` is `waiting`,
  `running`, `succeeded` (its agent opened a pull request), `merged`, `failed`, or `closed` (closed before a build
  started). Its `duration_ms` and `cost_usd` are summed over the builds. A running issue has `activity`: its agent's
  last action and last test result in words, when its log last grew, and its recent actions.
- `builds`: oldest first. `running_build` is the one running, if any.
- `elapsed_ms` and `cost_usd` are summed over the builds so far; a running build's time counts to its latest event.

An unknown run is an error.

### `list`

The runs, newest activity first.

```json
{"type": "list", "limit": 5}
```

`limit` is optional and defaults to 20. The reply holds each run's state, as `status` gives it, without `type`:

```json
{"type": "runs", "runs": [{"run": "owner/name#356", "disposition": "building", ...}]}
```

### `rerun`

Starts a new build of a run that needs attention, with the branch and plugin its last build had. The build skips the
issues already closed.

```json
{"type": "rerun", "run": "owner/name#356"}
```

The reply is `started`, as for `build`. A run that is building or waiting for review is an error.

### `delete`

Deletes a run that needs attention: every build's clone and record. GitHub keeps the outcome, so there is no
confirmation.

```json
{"type": "delete", "run": "owner/name#356"}
```

```json
{"type": "deleted", "run": "owner/name#356"}
```

A run that is building or waiting for review is an error.

### `ping`

Whether the app is there, and its version.

```json
{"type": "ping"}
```

```json
{"type": "pong", "version": "0.1.0"}
```
