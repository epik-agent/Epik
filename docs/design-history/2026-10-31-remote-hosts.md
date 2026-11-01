# Builds can run on another machine, which is a headless Epik app reached over SSH

- Status: Accepted
- Date: 2026-10-31
- Builds on: [An Epik app runs builds, one agent per issue, and records every run](2026-10-28-epik-runs-the-epic.md),
  and [A run is whatever it takes to finish a block of work](2026-10-28-a-run-finishes-a-block-of-work.md)
- Supersedes: the "host" half of §6 ("Two seams, each with one
  implementation") of "An Epik app runs builds, one agent per issue, and records every run", and the consequence "The laptop
  stays awake during builds" of [Builds run locally, and an epic is its sub-issues](2026-10-26-local-builds.md)

## Context

Builds run on the user's machine, and closing the lid stops them. That was
accepted in October as the price of running builds locally. It is the
complaint remote work exists to answer: **a build should go on while the
laptop is closed**, and the user should still see it in the Epik app when
the laptop opens again.

The October record "An Epik app runs builds, one agent per issue, and records every run" named a **host** seam, "run this command in this
directory and stream its output back", so that a remote host had somewhere
to go. Taken literally, that design leaves the orchestrator on the laptop
and moves only the agents, so closing the lid still ends the build. Moving
the agents alone doesn't solve the problem.

### Findings (2026-10-30, from documentation and this Mac)

| Question | Finding |
|---|---|
| Can a program start a new Claude Code cloud session now? | **Not in a documented way.** `claude --cloud "task"` creates one interactively. `claude -p … --cloud <id>` only queues a message into an existing session. Both need a claude.ai login, not an API key or `setup-token`. |
| Would a cloud session run an Epik build if it could? | **No.** In cloud sessions GitHub's proxy refuses GraphQL, so `gh pr` and `gh issue` fail. Epik merges with `gh`. Repository-declared plugins don't load. Account-synced plugins do. |
| Routines? | Still a per-task bearer token that has to be made by hand on the web, in research preview. The objection in "Builds run locally, and an epic is its sub-issues" stands. |
| Is `ssh` everywhere Epik runs? | **Yes.** macOS ships OpenSSH (10.3 here). Windows 10 1809+ and 11 ship the OpenSSH client. Linux has it. |
| Can a desktop program talk to a remote daemon over SSH without forwarding sockets? | **Yes, by a stdio bridge:** `ssh -T host <program> <bridge>`, as `docker -H ssh://` does with `docker system dial-stdio`. Forwarding a Unix socket (`ssh -L a.sock:b.sock`) is unreliable with Windows' OpenSSH. |
| Does piping the agent's ACP over `ssh` survive a dropped link? | **No.** Without a pty the remote processes get no SIGHUP and are orphaned, and the build is lost either way. |
| Does the Epik app run on a server with no desktop? | **Its `--headless` mode does no desktop work, but the binary links Tauri.** Only `epik-app` does (`tauri` with `tray-icon`, `tauri-plugin-dialog`, and `tauri-build`); `epik-core` has no Tauri, GTK or WebKit beneath it, `epik-view` is plain types, and `epik-ui` is WebAssembly embedded as files. On Linux, Tauri links `webkit2gtk-4.1`, `gtk3`, `libsoup3` and `libayatana-appindicator` as shared libraries, which the loader wants before `main` runs, so on a bare server the binary doesn't start. macOS and Windows have them as system frameworks. Tauri has no headless feature; a Cargo feature on `epik-app` is the standard way, and it also drops `tauri-build`, so a headless build needs no `trunk`. |
| Keeping a daemon alive on Linux | `systemd --user` unit plus `loginctl enable-linger`, the standard practice. |
| Signing in on a remote machine | `gh auth login` (one-time code, works over SSH) then `gh auth setup-git`; `claude auth login` (paste-back code, works over SSH). `claude setup-token` gives a model-only token without plugin sync, which builds don't need once the procedure travels with the prompt (see the record "Agents and roles: each role is an agent and a model"). |

## Decision

### 1. A remote host is a whole Epik app on another machine

A **host** is a machine that runs builds. **This computer** is always a
host. A **remote host** is another machine running `epik-app --headless`,
with its own `~/.epik`, its own `gh`, Claude Code and Node, signed in as
the user. A build that runs on a remote host runs entirely there: clone,
agents, merges, record, keeping awake, reconcile, and the closing comment.
The laptop can sleep or shut down; the build doesn't notice.

The remote host is any machine the user can reach with `ssh`: a server, a
cloud VM, a desktop at home. Epik names it by its SSH destination as the
user would type it (`devbox`, `me@build.example.com`), so `~/.ssh/config`,
keys, agents and jump hosts all work as they do for the user. Epik holds no
keys or passwords and opens no ports.

### 2. The local app talks to a host through the same protocol

A build request forwarded to a host carries the roles it was started with,
from the local settings, since the host has settings of its own.

The local app reaches a remote host by running
`ssh -T -o BatchMode=yes <host> ~/.epik/bin/epik-app stdio`. `stdio` reads
request lines from stdin, hands each to that machine's app over its socket,
and writes the replies to stdout, until stdin closes. Nothing new listens
anywhere: SSH is the authentication, and the socket keeps its `0600`.

`BatchMode` means Epik never waits for a password prompt, and never
answers a host-key question: a host the user hasn't connected to by hand
once is reported as unreachable, with SSH's own message, and the docs say
to run `ssh <host>` first. One `ssh` process serves a host, and the
requests over it, from the mirror and from forwarded builds alike, are
sent one at a time, each waiting for its reply. Whatever the remote login
shell prints before `epik-app stdio` starts is skipped: a line that isn't
JSON is not a reply.

The protocol (`docs/app-protocol.md`) is the contract both ways. It gains:

- `build` with an optional `host`. The local app forwards it.
- `runs`, `files` and `read`, which list every run, list a run's record
  files, and return the whole lines appended to one since a byte offset, so
  a record can be mirrored.
- `host` on `rerun`, `delete` and `status`, which the local app forwards.

### 3. The local app mirrors remote runs, and reads them like its own

The local app keeps a copy of each remote host's records under
`~/.epik/hosts/<host>/runs/`, in the same layout as `~/.epik/runs`. While
a remote run is building, it fetches what has been appended every few
seconds. Otherwise it checks every minute. A run that has left the host
leaves the mirror. Everything that reads runs, from the runs table and run
page to the live stream and `build_status`, reads the mirror through the
code that reads local runs. **Where a run is building is always shown**:
every place a run appears, the runs table, the tray menu, the run page,
the live stream's header, `build_status` and `build_list`, names its host
("Building on devbox"; "This computer" is never abbreviated away when
there is more than one host), and a host that can't be reached says when
it was last heard from. A run is named by its host and
`owner/name#number`. A repository built on a host is
added to the local monitored repositories too.

The mirror is a cache. The host owns the run: **Rerun** and **Delete** go
to the host, and the mirror never folds, interrupts, cleans or comments on
anything. When the laptop has been closed, the mirror catches up the next
time it opens.

### 4. The user picks where to build

Settings gains a **Hosts** section: add a host by its SSH destination,
see whether it is reachable and whether its setup is complete (its own
`setup` items, read over the bridge), and remove it. One host is the
**default for builds**, **This computer** unless the user picks another.
`build_start` takes an optional `host`, so the user can say "build #12 on
devbox" in the conversation.

### 5. A headless build of the app, for servers

`epik-app` built without its default `desktop` feature leaves out Tauri,
the windows, and keeping the machine awake (a server doesn't sleep): it
has only `--headless`, `mcp`, `stdio` and `--version`.
The release attaches it for Linux on x86-64 and ARM64 as tarballs with
checksums (`epik-app-linux-x64.tar.gz`, `epik-app-linux-arm64.tar.gz`).
Setting up a host is done by hand for now: download, put it at
`~/.epik/bin/epik-app`, run `epik-app setup` to check and install `gh`,
Node and Codex, sign in to `gh` and the agents over SSH, and run it as a
`systemd --user` service. The docs give the commands and the unit file.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| Orchestrate on the laptop; run only the agents remotely (`ssh host npx …`), as §6 of "An Epik app runs builds, one agent per issue, and records every run" sketched | Rejected | Closing the lid still ends the build, and a dropped link orphans the agents. |
| Claude Code cloud sessions as the host | Rejected | No documented way for a program to start one, and `gh pr` fails there. Revisit if both change. |
| Forward the remote socket (`ssh -L`) | Rejected | Unreliable with Windows' OpenSSH, and leaves a socket file to clean up. The stdio bridge needs nothing on either end. |
| A Rust SSH library (`russh`, `ssh2`) | Rejected | Re-implements config, agents, known hosts and jump hosts that the user's `ssh` already handles. |
| An HTTPS endpoint on the host | Rejected | A port, TLS and a credential to manage. |
| rsync or `tail -f` for the mirror | Rejected | A second channel, and no rsync on Windows. |
| Automating host setup (install, sign-in, service) from the local app | Deferred | Sign-ins are interactive anyway. Do it by hand until users have done it a few times. |
| A separate `epikd` program for servers | Rejected | One program built two ways keeps "the Epik app is Epik's only program". |
| Running the remote app on macOS or Windows hosts | Deferred | Linux servers and VMs are the common case. The headless build compiles everywhere; only the release artifact is Linux-only. |

## Consequences

- **The laptop can close during a remote build.** Local builds still keep
  it awake, as before.
- **A second copy of every remote run** sits under `~/.epik/hosts`, deleted
  with the run, and with `~/.epik` on uninstall. Like `~/.epik/runs`, it is
  transient.
- **The protocol becomes a remote contract**, used across versions. The
  local app checks the host's version with `ping` and refuses a host whose
  app is a different minor version, saying which to install.
- **Setting up a host is manual**, documented, and checked: the Hosts
  section says what is missing on the host, with the command to run.
- **The host's GitHub identity is the user's**, through `gh` on the host.
  Comments and merges look the same as from the laptop.

## Open questions

- Does `loginctl enable-linger` need an administrator on the distributions
  users pick? If it does, the docs say so; Epik still asks for no password.
- Is a few seconds' polling fast enough for the live stream to feel live? If
  not, a `follow` request that holds the bridge open can replace polling
  later.
- Should one run be allowed to move between hosts (rerun elsewhere)? Not
  now: a run's builds stay on the host of its first build.
