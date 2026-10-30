# Epik

_You say it. We make it._

Epik is a software-design partner that turns a conversation into working code.
Talk through what you want to build; Epik writes the plan up as GitHub issues,
then Claude Code builds them on your machine: it writes the code, commits it,
and opens a pull request for you to review.

## Install

The one thing to have beforehand is a GitHub account.

Download the installer for your computer from
[epik-agent.dev](https://www.epik-agent.dev/#download), or from the
[releases](https://github.com/epik-agent/Epik/releases): `Epik.dmg` for
macOS, `Epik.msi` for Windows, and `Epik_amd64.AppImage` or
`Epik_amd64.deb` for Linux. Open it, then open Epik, and its **Set up Epik**
checklist walks you through the rest: it gets the GitHub CLI, Claude Code
and Node, signs you in to GitHub and Claude, and installs Epik's plugin.

On Windows the installer isn't signed yet, so SmartScreen warns the first
time you open it.

## Getting started

Start Claude Code, in the terminal or in Claude Desktop's **Code** tab with
**My Computer** selected, and say "Hello, Epik."

Ask Epik how it works, how builds run, where their records are, or what to
do when something goes wrong.

## Uninstalling

Open Epik's Settings, press **Uninstall Epik…**, then delete the app. This
also deletes Epik's run history; Claude Code, `gh` and their sign-ins stay.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).
