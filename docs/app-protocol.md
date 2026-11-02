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
{"type": "build", "repo": "owner/name", "number": 356, "branch": "daemon"}
```

- `repo`: the repository, `owner/name`.
- `number`: the epic or issue. An issue with sub-issues is built as an epic.
- `branch` (optional): the branch the issues' pull requests merge into. By default `epic/<number>-<slug of title>` for
  an epic, and the repository's default branch for a single issue.
- `roles` (optional): the agent and model each role runs with, in the shape of `~/.epik/settings.json`:
  `{"coding": {"agent": "claude", "model": "opus"}, "review": null}`. `review` is `null` while review is off, and a
  `null` model is the agent's default. Without `roles`, the build runs with the app's settings. A build forwarded from
  another machine sends the roles it was started with.
- `host` (optional): the remote host to build on, by its SSH destination as listed in Settings (`devbox`). Without
  `host`, the build goes to the default host, which is this computer unless the user picked another. A host that isn't
  listed is an error.

For a remote host, the app sends the request on to that host's app over `ssh`, without `host`, and with `roles` set:
those given, or else this app's settings, since the host has settings of its own. The host checks the build as it
would its own, and its refusal is the error, naming the host:
`couldn't build owner/name#356 on devbox: the coding agent can't run: …`. Once the host has started the build, this app
adds the repository to its monitored repositories and syncs its mirror of the host's runs at once.

A run is built on one host only: while `owner/name#356` has a run on this computer or another host, building it
elsewhere is an error naming where it is: `owner/name#356 is on devbox: rerun or delete it there`.

Before anything is recorded, the app opens a session, without a prompt, with the coding agent and, when review is on,
the review agent, to check that each runs unattended and offers its model. An agent that can't refuses the build with an
error naming the agent, the model and the models it offers, as a missing setup item does. The build's roles are
recorded in its `build_started`; a record from before roles ran Claude Code with its default model and no review.

The app replies once the build has started: the issue is read, the repository cloned, and `build_started` written to
the build's record. That can take as long as the clone. The build then goes on in the app. When the epic issue already
has a run, the build is added to it; while one of its builds is running, another is refused. Each issue's agent is
prompted with the issue, the target branch, Epik's rules and the issue procedure, so it loads no plugin. A `plugin_dir`
sent by an older client is ignored.

```json
{"type": "started", "run": "owner/name#356", "build_id": "20261028T212859Z-k3f9", "kind": "epic", "branch": "daemon", "build_dir": "/Users/me/.epik/runs/owner/name/356/20261028T212859Z-k3f9"}
```

- `build_id`: the build's id, unique within its run: the UTC time it started and 4 random characters.
- `kind`: `epic` or `issue`.
- `build_dir`: the build's record, holding `events.jsonl`, each agent session's log, and `summary.json` once the
  build ends. For a build on a remote host it is the path there.
- `host` (only for a remote host): the host building it, `"host": "devbox"`.

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
      "cost_usd": 1.12,
      "sessions": [
        {
          "build_id": "20261028T212859Z-k3f9",
          "role": "coding",
          "round": 1,
          "agent": "claude",
          "model": "opus",
          "log": "357.jsonl",
          "started_at": "2026-10-28T21:29:01Z",
          "duration_ms": 512000,
          "cost_usd": 1.12
        }
      ]
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
      "sessions": [
        {
          "build_id": "20261028T212859Z-k3f9",
          "role": "coding",
          "round": 1,
          "agent": "claude",
          "model": "opus",
          "log": "358.jsonl",
          "started_at": "2026-10-28T21:37:35Z",
          "duration_ms": null,
          "cost_usd": null
        }
      ],
      "activity": {
        "last_action": "Running cargo test",
        "last_test": "23 passed, 1 failed",
        "last_event": "2026-10-28T21:38:57Z",
        "recent": ["Reading src/record.rs", "Editing src/record.rs", "Running cargo test"]
      }
    }
  ],
  "sessions": [],
  "builds": [
    {
      "build_id": "20261028T212859Z-k3f9",
      "status": "running",
      "reason": null,
      "pull_request": null,
      "started_at": "2026-10-28T21:28:59Z",
      "ended_at": null,
      "elapsed_ms": 600000,
      "cost_usd": 1.12,
      "cost_unreported": 0
    }
  ],
  "running_build": "20261028T212859Z-k3f9",
  "started_at": "2026-10-28T21:28:59Z",
  "latest_at": "2026-10-28T21:38:59Z",
  "elapsed_ms": 600000,
  "cost_usd": 1.12,
  "cost_unreported": 0
}
```

- `disposition`: `building` while a build runs; `reviewing` while a running build reviews the run's pull request;
  `waiting_for_review` when every issue merged and the pull request into the default branch waits for the user;
  `needs_attention` when the latest build failed or was interrupted, or its pull request was closed without merging.
  A build fails, with its pull request left for the user, when the review still asks for changes after its second
  round. `reason` says why a run needs attention.
- `pull_request`: the latest build's pull request into the default branch: the epic's, or a single issue's.
- `review` and `open_threads`, when review was on and the latest build reviewed its pull request: what its last review
  concluded, `approve`, `changes`, or `none` when the reviewer left no verdict; and how many of the pull request's
  review threads were unresolved when the build finished. Absent otherwise.
- `issues`: every issue of the run, including ones merged in earlier builds. Each one's `status` is `waiting`,
  `running`, `succeeded` (its agent opened a pull request), `merged`, `failed`, or `closed` (closed before a build
  started). Its `sessions` are the agent sessions it took over every build, oldest first, and its `duration_ms` and
  `cost_usd` are their sums; an issue recorded before sessions has none, and its agent's time and cost. A running
  issue has `activity`: its latest session's last action and last test result in words, when its log last grew, and
  its recent actions.
- Each session has its `role`, `coding` or `review`, and its `round`, from 1 for each role; the `agent` and `model`
  it ran (`model` is `null` for the agent's default); its `log`, a file name in its build's directory, `build_id`;
  when it started; and its `duration_ms` and `cost_usd`. `duration_ms` is `null` while it runs, and `cost_usd` when
  its agent reported no cost, as Codex never does: an unreported cost is never zero.
- `sessions`: the run's own sessions, such as reviews of its pull request and the coding sessions that address them,
  over every build. A single issue's run
  has these in its issue's `sessions`.
- `builds`: oldest first. `running_build` is the one running, if any.
- `elapsed_ms` and `cost_usd` are summed over the builds so far; a running build's time counts to its latest event.
  A build's `cost_usd` is its issues' and its own sessions'. Each `cost_usd` is what was reported; `cost_unreported`
  counts the issues with a finished session that reported no cost, which it lacks, the run's own sessions counting
  as its epic issue's.

An unknown run is an error.

`host` (optional) names the remote host the run is on, as `build` does. Without it, the run is this computer's. For a
remote host, the app sends the request on to that host's app, without `host`, and its reply is the host's:
`{"type": "status", "run": "owner/name#356", "host": "devbox"}`. A host that isn't listed or can't be reached is an
error naming it: `couldn't read owner/name#356 on devbox: …`.

### `list`

The runs, this computer's and every remote host's, newest activity first. A remote host's are read from this app's
mirror of them.

```json
{"type": "list", "limit": 5}
```

`limit` is optional and defaults to 20. The reply holds each run's state, as `status` gives it, without `type`:

```json
{"type": "runs", "runs": [{"run": "owner/name#356", "disposition": "building", ...}, {"run": "owner/name#357", "host": "devbox", ...}]}
```

A remote host's run has `host`, the host it is on. This computer's has none.

### `runs`

Every run's key, with no limit, and when it last changed: what a client mirroring the app's runs needs to tell which
runs it still has.

```json
{"type": "runs"}
```

```json
{"type": "run_keys", "runs": [{"run": "owner/name#356", "latest_at": "2026-10-28T21:38:59Z"}, ...]}
```

`latest_at` is the run's, as `status` gives it. The runs are in order of their keys.

### `files`

Every regular file in a run's record directory, `~/.epik/runs/<owner>/<name>/<number>/`.

```json
{"type": "files", "run": "owner/name#356"}
```

```json
{"type": "files", "run": "owner/name#356", "files": [{"path": "20261028T212859Z-k3f9/events.jsonl", "size": 5678}, ...]}
```

Each `path` is relative to the run's directory, separated by `/`, and each `size` is in bytes. Symbolic links aren't
followed. An unknown run is an error.

### `read`

The whole lines appended to one of a run's record files since a byte offset, so that a copy of the record can be kept
current.

```json
{"type": "read", "run": "owner/name#356", "path": "20261028T212859Z-k3f9/events.jsonl", "offset": 1234}
```

```json
{"type": "chunk", "path": "20261028T212859Z-k3f9/events.jsonl", "offset": 1234, "data": "...", "end": 2345, "size": 5678}
```

- `path`: as `files` gives it. A path that is empty, absolute, or climbs out of the run's directory with `..` is a
  `bad request`.
- `data`: whole lines from `offset`, up to 1 MiB, never part of a line. A last line without its newline is still being
  written, and is left for a later read. If the first line alone is over 1 MiB, it is returned whole. `data` is text,
  so bytes that aren't UTF-8, as in an agent's `.err` file, its raw standard error, are decoded lossily.
- `end`: the byte offset after `data`, from which to read next. Because of the lossy decoding, it is not always
  `offset` plus the length of `data`.
- `size`: the file's size in bytes. When `end` is less than `size`, there is more to read.

An offset at or past the size returns empty `data`. Past it, the file has been rewritten, and the client starts it
over from 0. An unknown run or file is an error.

### `rerun`

Starts a new build of a run that needs attention, with the branch and roles its last build had, whatever the settings
now say. The build skips the issues already closed.

```json
{"type": "rerun", "run": "owner/name#356"}
```

The reply is `started`, as for `build`. A run that is building or waiting for review is an error.

`host` (optional) names the remote host the run is on: `{"type": "rerun", "run": "owner/name#356", "host": "devbox"}`.
Without it, the run is this computer's. For a remote host, the app sends the request on to that host's app, without
`host`, and the host reruns it with its last build's roles. The host's refusal is the error, naming the host:
`couldn't rerun owner/name#356 on devbox: a build of owner/name#356 is running`. Once the host has started the build,
the reply is its `started` with `host`, and this app syncs its mirror of the host's runs at once.

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

`host` (optional) names the remote host the run is on, as for `rerun`. The host deletes the run, and this app syncs its
mirror at once, which removes the run here too. The host's refusal is the error, naming the host:
`couldn't delete owner/name#356 on devbox: …`.

### `setup`

Whether everything Epik needs is in place.

```json
{"type": "setup"}
```

```json
{"type": "setup", "items": [{"item": "gh", "title": "GitHub CLI", "state": "done", "detail": "gh 2.102.0", "usable": true}, {"item": "gh-auth", "title": "GitHub sign-in", "state": "missing", "detail": "signed in without the project scope", "usable": false}, ...]}
```

The items, in order, are `gh`, `gh-auth`, `claude`, `claude-auth`, `node` and `plugin`. Each one's `state` is `done`,
`missing` or `outdated`, and `detail` says more in a line. `usable` says whether Epik can use it as it is: a private
copy behind the latest release is outdated but usable. A `build` or `rerun` is refused while any item but `plugin` isn't
usable, with an error naming them.

### `ping`

Whether the app is there, and its version.

```json
{"type": "ping"}
```

```json
{"type": "pong", "version": "0.3.0"}
```

## Reaching an app on another machine

`epik-app stdio` carries the protocol over its stdin and stdout, so a client reaches the app on another machine by
running it there, as `ssh host epik-app stdio`. A command works with every `ssh`, where forwarding a socket doesn't with
Windows' OpenSSH.

Each line it reads is a request, sent as it is to the app on its own machine over a connection of its own. Each reply is
written to stdout as one line and flushed, in the order the requests came. Nothing else goes to stdout. It stops at the
end of its input.

`stdio` doesn't start the app. While the app isn't running, each request is answered with an error saying so:

```json
{"type": "error", "message": "no Epik app is running at /home/me/.epik/epik.sock on this machine: start it with `epik-app --headless`, or start its service"}
```
