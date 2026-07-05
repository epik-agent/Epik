# Epik

You say it. We build it.

Manager-mode feature development on GitHub: converge on a design, author the feature's issue graph, and launch autonomous builds on Claude Code on the web.

## Layout

- [`mcp/`](mcp/README.md) — **EpikMCP**, the GitHub mechanism: an MCP server that authors the issue graph and reads status.
- [`plugin/`](plugin/README.md) — the **epik** Claude Code plugin, the policy layer: commands, hooks, and the declaration of the EpikMCP server.

## Install

The repo root is a single-entry plugin marketplace:

```
/plugin marketplace add epik-agent/Epik
/plugin install epik@epik
```
