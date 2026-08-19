# ADR: Building from chat — git always, GitHub never

- **Status:** Accepted, and built
- **Date:** 2026-08-17
- **Source:** Design conversation (Cowork), August 2026
- **Note:** Reconstructed 2026-08-19. The original was delivered as a chat
  attachment and lost before check-in. Rebuilt from session records and checked
  against the shipped implementation on `rewrite`; the decisions are as
  discussed on 08-17, but the wording is not the original's.

## Context

The step before building an issue tree is building from a sentence: the user
describes what they want, the persona launches a coding Agent, and GitHub is
uninvolved. The Agent machinery existed — the runner, the `Agent` trait,
`ClaudeCode` — and its only consumers were tests.

The question that shaped the slice was what a build's destination is when there
is no repository yet.

## Decisions

### Git is always present

There is no plain-directory build destination. A directory is a *view* of a ref,
and concurrency plus merge-as-conflict-mechanism make every destination a ref —
so at minimum a build lands in a local bare git repository. What the simple mode
removes is GitHub and issues, not git.

The user never hears the word. Everything in Epik must be discoverable through
the chat or a standard configuration surface, so "git" is machinery, not
vocabulary the user is asked to learn.

The corollary: **a repository is never commitless.** `git_init` is a new verb on
the git allowlist, creating a bare repository on `main` with an empty root
commit, so it is immediately ready to be built in or cloned. A repository with
no commits has no base to branch from, and every later step assumes one.

Rejected: separate git and non-git modes; directory destinations. Materializing
a ref at a path would be a separate export verb if it were ever wanted, and it
has not been.

### One build interface, now and for the GitHub future

```
start_build(prompt, repository, branch, base?)
```

`repository` is a git URL, of which a local path is one — the erased
local/remote distinction doing its work. **The tool's caller names the branch**,
whether that caller is a human speaking through the persona or, later, machinery
naming `issue-42`. Prompt-sourcing is likewise the caller's job: the persona
relays chat text today, and an issue-implementing tool will fetch an issue body
later. The tool never changes; only its callers grow.

### Work happens in provisioned worktrees, never anyone's checkout

Each build gets a fresh worktree under the system temp directory, on its own new
branch at the base. Worktrees give sibling isolation for free — one object
store, one branch each — which is the property the concurrent future needs.

Provisioning sets the Epik persona's identity per-worktree through
`extensions.worktreeConfig`, so **the attribution invariant is enforced by
provisioning and never hoped for from the Agent.** The repository's own config is
never given the persona's identity.

A local repository means no push step at all: commits land in the ref store as
they are made.

### Epik observes; the Agent commits

The commit contract is deliberately thin. Claude Code commits its own work, and
Epik assumes it succeeded. At reap, Epik *observes*: did the branch advance past
the base commit, and is the worktree clean. A clean worktree is removed; a dirty
one is left where it is and its path noted.

Salvage sweeps, distinguishable salvage commits, and failure remediation are
deliberately unbuilt. That code path gets designed against a real corpse, not
against a hypothesis about what the corpses will look like.

### Remote sync is the launcher's, and is unbuilt

The coding Agent never needs network credentials. Pushing a branch to a remote is
launcher policy for when GitHub returns. Until then, remote sources compose
through the persona's existing verbs: clone, build locally, push.

### A modal is just a different kind of incoming message

The persona sometimes needs something from the user mid-turn — where a
repository should live, most immediately. This is modelled on the same modal
behaviour Claude Desktop has, and the insight that makes it cheap is that a
question is not a third kind of IPC traffic. There are two channels already:
events out, commands in.

- The question is `TranscriptItem::Question { id, ask }` on the existing event
  channel. `Ask` is **modality-free** — the backend says *what* it needs and
  never how to collect it.
- The answer is an `answer_question` command, correlated by id.
- The resolution round-trips as an event, `QuestionResolved`, appended and
  emitted exactly like a tool call and its result. The frontend never shows
  anything it did not receive as an event; fold discipline holds.
- The turn suspends while a question is pending. Which thread that blocks on is
  an implementation detail.
- **A decline is a first-class answer**, read and reacted to by the persona, not
  an error.

### Questions are cards

The one-card family gains a lifecycle: pending and interactive, then resolved
and inert — a record of what was asked and what was said.

Every input modality lives in the **frontend**. Today that is a path field, a
Browse button invoking the native save-as dialog frontend-side, and a decline. In
a detached daemon the same card would render a backend-fed listing, and the
backend code would be identical.

This is the seam a future permission layer reuses — *may I push?* is a question
card — and it is exactly why the modal must never be a native dialog raised by
the backend.

### The picker is a fallback, not a gate

A user who types a location in text has already answered, and no question is
asked. There is no `~/Epik` default projects root: the founder knows what a file
system is, and the save-as dialog opens where the OS thinks it should. Epik owns
no directory on the user's disk.

### Progress is read from git, not narrated into chat

Agent events never enter the chat transcript; the 2026-08-08 decision stands. Run
events fold into an in-memory run record — the first consumer of the Agent event
channel — which a `build_status` tool reads. Otherwise the persona reads
repository state through the git verbs, `git_log` on the branch chief among them.

GitHub-state-as-the-persona's-view generalizes to **git-state-as-view**.

One run is in flight at a time, the same slot discipline as one turn in flight. A
branch that already exists is refused in git's own words by provisioning — the
caller's error, reported rather than worked around.

## Consequences

- The Agent's preamble forbids pushing, creating or switching branches, and
  touching git configuration. The identity and the branch are provisioned, not
  requested.
- Archival log sinks remain unbuilt. The run record is memory only, and dies
  with the process.
- The question seam is the most reusable thing this slice produced, and its
  first non-repository consumer will likely be permissions.
