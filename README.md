# Epik

_You say it. We build it._

Manager-mode feature development on GitHub: converge on a design, author the feature's issue graph, and launch autonomous builds on Claude Code on the web.

## Layout

- [`mcp/`](mcp/README.md) — **EpikMCP**, the GitHub mechanism: an MCP server that authors the issue graph and reads status.
- [`plugin/`](plugin/README.md) — the **epik** Claude Code plugin, the policy layer: commands, hooks, and the declaration of the EpikMCP server.

## Installation

Epik is a plugin that brings an MCP server along with it. You do not install the server separately — installing the plugin is enough. (The only exception is Claude Desktop / CoWork, which can't load plugins; see [Using Epik from Claude Desktop / CoWork](#using-epik-from-claude-desktop--cowork) below.)

### Prerequisites

Work through these in order. Each has a check you can run to confirm it before moving on.

1. **Claude Code**, current version. Check: start `claude` and type `/plugin` — the command should exist. If it doesn't, update Claude Code first.

2. **The `gh` CLI, installed and logged in.** Epik does all of its GitHub reading and writing through `gh`, using whatever account you log it into.

   ```bash
   # install (macOS)
   brew install gh

   # log in — follow the prompts
   gh auth login

   # check
   gh auth status
   ```

   You should see `✓ Logged in to github.com account <you>`.

3. **`uv`**, the Python package runner. The plugin uses its `uvx` command to download and run the EpikMCP server automatically — you never install the server yourself.

   ```bash
   # install (macOS)
   brew install uv

   # check
   uvx --version
   ```

### Install the plugin

Run these two commands **inside a Claude Code session** (they are slash commands, not shell commands):

```
/plugin marketplace add epik-agent/Epik
```

This registers this GitHub repository as a plugin *marketplace* — a catalog Claude Code can install plugins from. `epik-agent/Epik` is the repository's GitHub `owner/name`; it's the same for everyone (only change it if you're installing from your own fork). You should see:

```
Successfully added marketplace: epik
```

Then:

```
/plugin install epik@epik
```

The form is `plugin-name@marketplace-name`. This repo is a single-entry marketplace named `epik` containing one plugin also named `epik` — hence the doubled name. (Both names come from [`.claude-plugin/marketplace.json`](.claude-plugin/marketplace.json) in this repo, not from anything on your machine.) You should see:

```
✓ Installed epik. Run /reload-plugins to apply.
```

Do what it says:

```
/reload-plugins
```

### Verify it worked

In the same session:

- `/help` lists two new commands, **`/epik:feature`** and **`/epik:issue`**.
- Ask Claude something like *"list the open issues in this repo"* — it should answer via the `EpikMCP` tools (you'll see tool calls named `mcp__…EpikMCP__issue_list` and similar).

That's it for Claude Code. Day-to-day usage is described in the [plugin README](plugin/README.md#usage).

### Updating

When a new version of Epik is pushed to GitHub:

```
/plugin marketplace update epik
/reload-plugins
```

### Uninstalling

```
/plugin uninstall epik@epik
/plugin marketplace remove epik
```

## Using Epik from Claude Desktop / CoWork

Claude Desktop (including CoWork sessions) doesn't load Claude Code plugins, so the plugin install above does nothing for it. Instead, register the EpikMCP server directly in the desktop app's config file. CoWork sessions then reach it as a bridged local tool.

1. **Find the config file:**
   - macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
   - Windows: `%APPDATA%\Claude\claude_desktop_config.json`

2. **Find the absolute path to `uvx`:**

   ```bash
   which uvx
   ```

   You need the absolute path because the desktop app launches servers with a minimal `PATH` that usually doesn't include wherever `uvx` lives.

3. **Add EpikMCP under `mcpServers`** (replace both `/path/to/uvx` placeholders with your answer from step 2, and keep any other servers already in the block):

   ```json
   {
     "mcpServers": {
       "EpikMCP": {
         "command": "/path/to/uvx",
         "args": [
           "--from",
           "git+https://github.com/epik-agent/Epik.git#subdirectory=mcp",
           "epik-mcp"
         ],
         "env": {
           "PATH": "/opt/homebrew/bin:/path/to/uvx-directory:/usr/bin:/bin:/usr/sbin:/sbin"
         }
       }
     }
   }
   ```

   The `env.PATH` line matters for the same minimal-`PATH` reason: it lets the server find the `gh` CLI (installed at `/opt/homebrew/bin/gh` by Homebrew on Apple Silicon). Include the directory containing `uvx` and the directory containing `gh`.

4. **Quit and relaunch the Claude desktop app**, then start a **new** CoWork task. Existing sessions don't pick up servers started after they connected.

To also launch cloud feature builds from Desktop/CoWork (the `feature_launch` tool), add `EPIK_ROUTINE_ID` and `EPIK_ROUTINE_TOKEN` to that `env` block — see [Build module setup](mcp/README.md#build-module-feature_launch) for how to create the routine and get those values. Everything else works without them.

## Troubleshooting

**`/plugin marketplace add` fails with `JSON Parse error: Property name must be a string literal`**
Claude Code's own marketplace registry file is corrupt (often a stray trailing comma). Check it:

```bash
python3 -m json.tool ~/.claude/plugins/known_marketplaces.json
```

If that reports an error, fix the syntax it points at (or delete the file and re-add your marketplaces). This file is Claude Code's, not Epik's — any marketplace command fails the same way until it parses.

**`/plugin marketplace add` says the name `epik` is already in use**
You added it before (perhaps from a local path). Run `/plugin marketplace remove epik`, then add it again.

**Commands installed but `EpikMCP` tools missing**
Run `/mcp` to see server status. The usual causes: `uv` isn't installed (the server is launched with `uvx`), or the machine can't reach GitHub to fetch the server package.

**GitHub reads fail even though the server is running**
The plan tools shell out to `gh`. Run `gh auth status` in a terminal; if it's not logged in, `gh auth login`.

**Desktop/CoWork: server never appears**
Almost always a path problem: use the absolute `uvx` path in `command`, set `env.PATH` as shown above, restart the app, and start a fresh session. The desktop app's MCP logs are in `~/Library/Logs/Claude/` on macOS.
