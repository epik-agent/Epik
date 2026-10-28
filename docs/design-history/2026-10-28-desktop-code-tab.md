# Claude Desktop's Code tab, in the My Computer environment, is supported

- Status: Accepted
- Date: 2026-10-28
- Supersedes: §1 ("The Claude Code terminal is the supported surface") of
  [terminal-first-gh-only](2026-10-28-terminal-first-gh-only.md), and its
  consequence "Desktop is unsupported until a later entry says otherwise"

## Context

The terminal-first entry deferred Desktop until the terminal setup worked
fully, and said Desktop returns with its own entry. The terminal setup now
works. This is that entry: it says where Epik works in Claude Desktop and
where it doesn't, so nobody picks an environment that can't load Epik and
concludes Epik is broken.

### Findings (2026-10-28, from use)

| Question | Finding |
|---|---|
| Does Desktop's Code tab, with the My Computer environment selected, run Epik? | **Yes.** It runs Claude Code on the user's Mac, with their user-scope plugins and a shell, so it is the same setup as the terminal and everything tested in the terminal carries over. |
| Does the Code tab's cloud environment load Epik? | **No.** Cloud sessions don't have the locally installed plugin, and the [one-plugin entry](2026-10-24-one-plugin-standard-github.md) found that cloud sessions switch plugin MCP servers off anyway. |
| Can Desktop's chat run Epik? | **Not without new tools.** Chat has no shell for `gh`, so supporting it would mean adding the seven issue tools (create, edit, comment, sub-issues) back to Epik's server. |

## Decision

Epik supports the Claude Code terminal and Desktop's Code tab in the My
Computer environment. The Code tab's cloud environment and Desktop's chat
are not supported.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| Support Desktop's chat | Deferred | It needs the issue tools back in Epik's server, and the Code tab already covers Desktop users. Revisit if someone asks for chat. |
| Support the Code tab's cloud environment | Rejected | Cloud sessions don't load the locally installed plugin and switch plugin MCP servers off. |

## Consequences

- **User setup is unchanged:** the same four steps as the terminal.
- **Epik's server still has no issue tools.** The terminal-first entry
  said they "come back when Desktop does". With Desktop supported through
  the Code tab, `gh` in the shell still covers them, so they stay out.
