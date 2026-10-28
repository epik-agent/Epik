# Terminal first, and `gh` is Epik's only GitHub login

- Status: Accepted
- Date: 2026-10-28
- Supersedes: §2 ("GitHub access comes from standard pieces") and §3
  ("Epik's server provides only what the default connector lacks") of
  [one-plugin-standard-github](2026-10-24-one-plugin-standard-github.md),
  and the parts of that entry that make Claude Desktop a supported surface

## Context

Two days of setup trouble came from GitHub access and from Desktop, not
from Epik's own work. Each fix was reasoned out in advance, and each one
turned up another surprise. The way forward is to get one simple setup
working end to end and to grow from there.

### Findings (2026-10-27 and 28, from use)

| Question | Finding |
|---|---|
| Does the plugin's declared GitHub server work in the terminal? | **No.** It shows as a failed server in every session: GitHub's sign-in server doesn't support dynamic client registration. |
| Is connecting Claude's GitHub connector enough to write? | **No.** It signs in through a GitHub App, Anthropic's *Claude Github MCP Connector*, which writes only to accounts where the app is installed. Reads of public repositories work without it, so the failure appears only on the first write, as `403 Resource not accessible by integration`. An organization owner has to install it for an organization. |
| Does `gh` reach an organization's repositories? | **Yes**, with no extra step: `gh auth login` was enough for `push` access. |
| Does a Desktop conversation started from a claude.ai Project load the locally installed plugin? | **No** (observed once). It loaded only the plugins installed on the claude.ai account. |
| What does auto mode do with a build's merges? | Merging into an epic branch is allowed. Merging into the default branch is blocked as "merge without review". |
| Does a terminal build of an epic work end to end? | **Yes.** Its sub-issues merged into the epic branch in dependency order, and the epic's pull request was left for the user. |

## Decision

### 1. The Claude Code terminal is the supported surface

Desktop is deferred until the terminal setup works fully. It returns
with its own entry.

### 2. `gh` is Epik's only GitHub login

The plugin declares only Epik's server. Epik doesn't use Claude's GitHub
connector or GitHub's hosted MCP server. Claude works with GitHub through
Epik's tools, `gh_raw`, and `gh` in the shell.

### 3. Builds never merge into the default branch

Sub-issue pull requests merge into the epic branch. A pull request into
the default branch waits for the user. The issue procedure says so, so the
behavior doesn't depend on auto mode's classifier.

### User setup

1. Node.
2. `gh`, signed in with `gh auth login -s project`, and `gh auth setup-git`
   so builds can push.
3. Claude Code, installed and signed in.
4. The plugin, installed at user scope.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| Keep the connector as a fallback | Rejected | A second GitHub login that fails silently until the first write, on any account where its app isn't installed. `gh_raw` is the fallback. |
| GitHub's official MCP server, run locally | Rejected | Needs a personal access token that the user creates and manages. |
| Support Desktop now | Deferred | Which Desktop conversations load the plugin is unsettled, and without the connector Desktop has no shell for `gh`. |

## Consequences

- **Setup is four steps**, all ordinary developer tooling.
- **Epik's server has no issue tools** (create, edit, comment, sub-issues).
  In the terminal, `gh` covers them. They come back when Desktop does.
- **Desktop is unsupported** until a later entry says otherwise.
