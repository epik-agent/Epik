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

## Build on another machine

A build on your computer stops when you close the lid. To build while it's
closed, build on a **host**: another machine you can reach with `ssh`, such
as a server, a cloud VM or a desktop at home, running Epik without its
windows. The build runs entirely there, and the Epik app on your computer
shows it as it goes and catches up when you open the lid. Hosts run Linux,
on x86-64 or ARM64.

Epik reaches a host by running `ssh <host>`, where `<host>` is what you'd
type yourself, such as `devbox` or `me@build.example.com`, so your
`~/.ssh/config`, keys and jump hosts work as they do for you. Epik never
answers a password or a host-key question, so first, from your computer:

```sh
ssh <host>
```

Accept its host key if asked, and make sure it doesn't ask for a password
(set up a key with `ssh-copy-id <host>` if it does). Do everything below
on the host, over that `ssh`.

**1. Install Epik.** Download the app and its checksum, check it, and put
the binary at `~/.epik/bin/epik-app`. On ARM64, use `linux-arm64` in place
of `linux-x64`. The host must run the same minor version of Epik as your
computer (`0.4.x` with `0.4.x`): `latest/download/` is the latest release,
and `download/v0.4.0/` is a particular one.

```sh
curl -fsSLO https://github.com/epik-agent/Epik/releases/latest/download/epik-app-linux-x64.tar.gz
curl -fsSLO https://github.com/epik-agent/Epik/releases/latest/download/epik-app-linux-x64.tar.gz.sha256
sha256sum -c epik-app-linux-x64.tar.gz.sha256
mkdir -p ~/.epik/bin
tar -xzf epik-app-linux-x64.tar.gz -C ~/.epik/bin
~/.epik/bin/epik-app --version
```

**2. Install the tools.** `epik-app setup` checks what a build needs, and
says what to run for each item that isn't done:

```sh
~/.epik/bin/epik-app setup
```

It installs private copies of the GitHub CLI and Node in `~/.epik/tools/`,
checked against their published checksums, with
`~/.epik/bin/epik-app setup install gh` and
`~/.epik/bin/epik-app setup install node`. Builds run Claude Code, so if
it's missing, install it with Anthropic's installer:

```sh
curl -fsSL https://claude.ai/install.sh | bash
```

The host doesn't need the Epik plugin: builds bring their own.

**3. Sign in.** Sign `gh` in to GitHub with the `project` scope, and let
`git` push with it. Over `ssh`, it gives you a one-time code to enter in
your browser. Where Epik installed its own `gh`, `epik-app setup` prints
the full path to type.

```sh
gh auth login -s project
gh auth setup-git
```

Then sign in each agent your roles use: a build on a host runs with the
roles in your computer's Settings. Claude Code gives you a code to paste
back:

```sh
claude auth login
```

Run `~/.epik/bin/epik-app setup` again. When everything a build needs is
done, it says so and exits successfully.

**4. Run it as a service.** Save this as
`~/.config/systemd/user/epik.service`:

```ini
[Unit]
Description=Epik, building on this machine
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=%h/.epik/bin/epik-app --headless
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Start it, and keep it running after you log out:

```sh
systemctl --user daemon-reload
systemctl --user enable --now epik
loginctl enable-linger "$USER"
```

If `loginctl enable-linger` asks for a password, your distribution wants an
administrator for it: have one run `sudo loginctl enable-linger <user>`.

**5. Add the host.** On your computer, open Epik's **Settings…**, and under
**Hosts** add the host as you'd type it after `ssh`. Settings says whether
it's reachable and whether its setup is complete. Make it the default for
builds there, or name it when you ask for a build: "build #12 on devbox".

### When a host isn't working

- **Unreachable.** Settings shows `ssh`'s own message. Check that
  `ssh <host>` works from your computer without a password or a host-key
  question, then that the service is running on the host, with
  `systemctl --user status epik` and `journalctl --user -u epik`.
- **Another version.** Settings says, for example, "devbox runs Epik
  0.3.2; install 0.4.x there". Download that release as in step 1, replace
  `~/.epik/bin/epik-app`, and run `systemctl --user restart epik`.
- **Setup incomplete.** Settings lists what is missing on the host, with
  the command to run. Run it on the host over `ssh`, or run
  `~/.epik/bin/epik-app setup` there to see everything.

## Uninstalling

Open Epik's Settings and press **Uninstall Epik…**. When it finishes, quit
Epik and remove the app the usual way for your platform: drag it to the Trash
on macOS, uninstall it from Installed apps on Windows, or delete the AppImage
or `sudo apt remove epik` on Linux. Uninstalling also deletes Epik's run
history; Claude Code, `gh` and their sign-ins stay.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).
