# Epik MCP Bundle (`.mcpb`)

One-click install of EpikMCP for Claude Desktop / Cowork. An `.mcpb` file is
a zip archive containing a local MCP server and a `manifest.json`; Claude
Desktop installs it on double-click — no `claude_desktop_config.json`
editing, no user-side Python or `uv`.

The manifest uses the **`uv` server type** (MCPB manifest v0.4): Claude
Desktop manages the Python runtime and installs the dependencies declared in
`pyproject.toml` itself. The only prerequisite left on the user's machine is
the `gh` CLI, logged in.

## Contents

- `manifest.json` — extension metadata, tool list, and launch config. The
  `version` here is the **extension version** (what users see in Claude
  Desktop), maintained independently of the `epik-mcp` package version in
  `../pyproject.toml`.
- `main.py` — bundle-only launcher. Appends the conventional `gh` install
  directories to the desktop app's minimal `PATH` before starting the
  server, replacing the hand-written `env.PATH` block the old JSON install
  required. Not part of the `epik_mcp` package.
- `icon.png` — rendered at 512×512 from
  `../../website/brand/logo-mark-light.svg` (e.g.
  `cairosvg logo-mark-light.svg -o icon.png --output-width 512 --output-height 512`).
  Re-render if the brand mark changes.
- `.mcpbignore` — keeps caches and virtualenvs out of the archive.
- `compatibility.claude_desktop` in the manifest pins the oldest Claude
  Desktop version the uv runtime has been verified on (no published
  support matrix exists); raise it as newer versions are tested.
- `build.py` — assembles a staging directory (epik-mcp source + the files
  above, dev dependencies stripped, fresh lock) and packs it with the MCPB
  CLI. Requires `uv` and `npx`.

## Building

Releases build themselves: pushing a `v*` tag runs the `release-mcpb` job in
[`ci.yml`](../../.github/workflows/ci.yml), which packs the bundle stamped
with the tag's version and attaches it to a GitHub release for that tag —
a pre-release when the version carries a suffix, so any number of release
candidates (`v1.0.0-rc.1`, `v1.0.0-rc.2`, …) can be cut and tested before
the final `v1.0.0`. The website's download button points at the release
asset.

To build locally (e.g. to verify exactly what you reviewed):

```
python mcp/mcpb/build.py            # version from manifest.json
python mcp/mcpb/build.py --version 1.0.0-rc.9   # stamped, staged copy only
```

Output lands in `mcp/mcpb/dist/` (gitignored). Requires `uv` and `npx`.

## References

- [Build a desktop extension with MCPB](https://claude.com/docs/connectors/building/mcpb)
- [MCPB manifest spec](https://github.com/modelcontextprotocol/mcpb/blob/main/MANIFEST.md)
- [MCPB CLI](https://github.com/modelcontextprotocol/mcpb/blob/main/CLI.md)
