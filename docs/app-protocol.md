# The Epik app's protocol

Other programs, chiefly Epik's MCP server, start builds and read their state by talking to the Epik app over a local
socket. This is the contract between them. The app's side is `epik_core::ipc`.

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

The app replies once the run has started: the issue is read, the repository cloned, and `run_started` written to the
run's record. That can take as long as the clone. The build then goes on in the app.

```json
{"type": "started", "run_id": "owner-name-356-20261028T212859Z-k3f9", "kind": "epic", "branch": "daemon", "run_dir": "/Users/me/.epik/runs/owner-name-356-20261028T212859Z-k3f9"}
```

- `kind`: `epic` or `issue`.
- `run_dir`: the run's record, holding `events.jsonl`, each issue's agent output, and `summary.json` once the run ends.

A pull request, a closed issue, or a failure before the run starts is an error, and leaves no record.

### `status`

A run's state.

```json
{"type": "status", "run_id": "owner-name-356-20261028T212859Z-k3f9"}
```

The reply is the run's state, folded from its events, with `"type": "status"`. It is what the run's `summary.json`
holds when the run ends.

```json
{
  "type": "status",
  "run_id": "owner-name-356-20261028T212859Z-k3f9",
  "repo": "owner/name",
  "number": 356,
  "title": "The Epik app runs builds",
  "kind": "epic",
  "branch": "daemon",
  "status": "running",
  "reason": null,
  "pull_request": null,
  "members": [
    {
      "number": 357,
      "title": "Rust workspace and CI",
      "blocked_by": [],
      "status": "merged",
      "reason": null,
      "pull_request": 367,
      "duration_ms": 512000,
      "cost_usd": 1.12
    }
  ],
  "started_at": "2026-10-28T21:28:59Z",
  "ended_at": null,
  "elapsed_ms": 600000,
  "cost_usd": 1.12
}
```

- `status`: `running`, `succeeded`, `failed` or `interrupted`. `reason` says why a run failed or was interrupted.
- `pull_request`: the epic's pull request, or a single issue's, once there is one.
- Each member's `status`: `waiting`, `running`, `succeeded` (its agent opened a pull request), `merged`, or `failed`.
- `elapsed_ms` runs to `ended_at`, or while running to the latest event. `cost_usd` sums the members' costs so far.

An unknown run is an error.

### `list`

The latest runs, newest first.

```json
{"type": "list", "limit": 5}
```

`limit` is optional and defaults to 20. The reply holds each run's state, as `status` gives it, without `type`:

```json
{"type": "runs", "runs": [{"run_id": "owner-name-356-20261028T212859Z-k3f9", "status": "running", ...}]}
```

### `ping`

Whether the app is there, and its version.

```json
{"type": "ping"}
```

```json
{"type": "pong", "version": "0.1.0"}
```
