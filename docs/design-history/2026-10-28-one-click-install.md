# Epik installs from one download, and the Epik app is its only program

- Status: Accepted
- Date: 2026-10-28
- Builds on: [epik-runs-the-epic](2026-10-28-epik-runs-the-epic.md)
- Supersedes: "User setup" in
  [terminal-first-gh-only](2026-10-28-terminal-first-gh-only.md), and the
  Node MCP server described in that entry and its predecessors

## Context

Setting up Epik is a list of manual steps in `README.md`, and 0.3.0 made the
list longer: the Epik app is installed with `cargo install`, so every user
now needs Rust as well as Node, `gh` with two sign-in commands, Claude Code,
and two `/plugin` commands typed in a Claude Code session. Each step is
ordinary developer tooling, and each is a place to get stuck. The goal for
0.3.0 is that a person clicks a download on Epik's website, opens what they
downloaded, and is guided through the rest.

The app also duplicates the plugin. Epik's MCP server is about 900 lines of
Node in `plugin/server`. Its dependency graph, `gh` calls and socket client
now have Rust twins in `epik-core` (`graph`, `github`, `ipc`), and Epik uses
Rust wherever it can.

### Findings (2026-10-28)

| Question | Finding |
|---|---|
| Can the plugin be installed without typing in a Claude Code session? | **Yes.** `claude plugin marketplace add epik-agent/Epik` and `claude plugin install epik@epik --scope user` run from a shell. `claude plugin marketplace update` and `claude plugin update` keep it current. |
| Can Claude Code be installed and checked from a program? | **Yes.** Anthropic's installers are `curl -fsSL https://claude.ai/install.sh \| bash` (macOS, Linux) and `irm https://claude.ai/install.ps1 \| iex` (Windows); the native install updates itself. `claude auth status` and `claude auth login` check and start the sign-in. |
| Can `gh` be installed without a package manager? | **Yes.** Its releases ship standalone archives for each platform, with checksums. `gh auth status`, `gh auth login --web -s project` and `gh auth setup-git` check and set up its sign-in. |
| Is the repository public? | **Yes**, so its GitHub Releases are downloadable without signing in. |
| Can a release be signed for macOS? | **Probably.** A Developer ID certificate is on hand (`apple-dev/Certificates.p12`); the `still-rust` CI was already written to sign and notarize when given secrets. |
| Does anything still need Node? | **The ACP adapter.** The app runs `claude-agent-acp` through `npx`. Removing the Node server doesn't remove Node. |

## Decision

### 1. The Epik app is Epik's only program

`epik-app mcp` serves Epik's MCP tools over stdio, using `rmcp` (the
official Rust MCP SDK) and `epik-core` directly. It offers the same tools
as today's Node server, with the same names and arguments, so the skills
don't change. The plugin's MCP configuration runs the app instead of Node,
and `plugin/server` is deleted.

The plugin becomes skills and configuration: nothing to build, bundle or
check against its source.

### 2. A fixed place to find the app

Each platform's installer puts the app somewhere different (`/Applications`
on macOS, Program Files on Windows, wherever a Linux user keeps an
AppImage). The app keeps a link to itself at `~/.epik/bin/epik-app`
(`epik-app.exe` on Windows), refreshed every time it starts. The plugin's
MCP configuration names that path. Claude Code and the app's own tools
never depend on the user's `PATH` to find Epik.

### 3. CI builds the installers

Pushing a version tag runs a release workflow that builds the app with
Tauri's bundler on each platform and attaches the results to a GitHub
Release:

- macOS: a universal `.dmg`, signed and notarized.
- Windows: an `.msi`, signed when a certificate is configured.
- Linux: an AppImage and a `.deb`.

Epik's website gets a download page whose buttons link to the latest
release's files for the visitor's platform. Users no longer need Rust.

### 4. The app sets up everything else, with the user's consent

On first launch, and whenever the user chooses "Check setup" in the tray
menu, the app shows a **setup checklist**. Each item says whether it is
done, and what doing it would involve:

1. **`gh`**: found on the machine, or installed by the app.
2. **GitHub sign-in**: `gh auth login --web -s project` (opens the
   browser), then `gh auth setup-git`.
3. **Claude Code**: found, or installed with Anthropic's installer.
4. **Claude sign-in**: `claude auth login` (opens the browser).
5. **Node**: found, or installed by the app. It runs the ACP adapter.
6. **The Epik plugin**: added and installed with `claude plugin`, or
   updated when it is behind.

Nothing is installed or changed until the user approves that item. The
checklist installs nothing system-wide and never asks for an
administrator's password.

Where a tool is already on the machine, the app uses it. Otherwise it
installs a private copy under `~/.epik/tools/`, downloaded from the tool's
own releases and checked against its published checksum before it runs.
Claude Code is the exception: its own installer is used, because it keeps
itself up to date.

The same checks run before every build, without the checklist: a build
that would fail for want of a tool or a sign-in fails at once and says
which.

### 5. `README.md` describes the download

Setup in the README becomes: download Epik, open it, follow the checklist.
Installing from source (`cargo install`) moves to a short section for
contributors.

### 6. Uninstalling is two steps, and the app doesn't remove itself

The app's Settings uninstall Epik: it removes the Epik plugin and
everything in `~/.epik`. Then the user quits the app and removes it the
way their platform removes any app, and the uninstall dialog says how.

Each platform's installer owns the app: the `.dmg` puts it in
`/Applications`, the `.msi` registers it with Windows, a `.deb` is owned by
dpkg and needs root to remove, and an AppImage lives wherever the user put
it. Removing the app any other way is non-standard and, on Windows, leaves
an orphaned installer registration. A running app also can't cleanly
remove itself: Windows locks the running `.exe`. So Epik removes `~/.epik`
and its plugin, and the platform removes the app. The dialog's last button
takes the user most of the way: it reveals the app in Finder or the file
manager, or opens Windows' Installed apps, then quits.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| A shell script (`curl … \| sh`) | Rejected | Not something to click on a web page, and a different script for Windows. |
| Homebrew, winget and Linux packages | Deferred | Good later channels for the same app. Not needed to remove the manual steps. |
| Bundle `gh`, Node and Claude Code inside the installer | Rejected | A larger download, copies that go stale, and Claude Code updates itself. |
| A separate Rust MCP server shipped in the plugin | Rejected | A second binary to build and distribute for every platform, when every user already installs the app. |
| Keep the Node server | Rejected | Two implementations of the graph, GitHub and socket code, which can drift apart. |
| Install tools system-wide | Rejected | Needs administrator rights, and touches the user's other tools. |
| Automatic app updates | Deferred | Tauri has an updater; until it is wired up, the checklist says when a newer release exists. |

## Consequences

- **Setup is one download** plus two browser sign-ins and a few approvals.
- **Node stays**, for the ACP adapter only, now installed by the app where
  it is missing.
- **CI holds signing secrets** for macOS, and later Windows. Until Windows
  has a certificate, SmartScreen warns on first run.
- **The app owns private copies of tools** under `~/.epik/tools/` and has
  to keep them current: the checklist checks their versions.
- **Every Epik user needs the app,** including one who only plans. They
  already did: 0.3.0's setup installs it.
- **The plugin and the app version together.** The app answers `ping` with
  its version; the plugin's skills name nothing version-specific.

## Open questions

- **How the plugin's MCP configuration names `~/.epik/bin` on every
  platform.** Plugin configuration substitutes `${CLAUDE_PLUGIN_ROOT}`;
  whether it expands `${HOME}` (and on Windows, `%USERPROFILE%`) needs
  checking. If it doesn't, a plugin user-config value set by the checklist
  is the fallback.
- **Windows code signing:** which certificate, and whether to ship unsigned
  until there is one.
- **Linux desktops without a tray:** the checklist needs a window, so the
  app shows it on first launch whether or not it has a tray.
- **Removing Node** would need the ACP adapter as a standalone binary. Not
  pursued here.
