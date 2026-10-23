#!/usr/bin/env bash
# Install Epik from this checkout for development, then check that the new
# EpikMCP is what runs.
#
#   scripts/dev-install.sh           report leftovers, install, smoke-test
#   scripts/dev-install.sh --check   report leftovers and smoke-test only
#
# Installs:
#   - Claude Code: this checkout as the `epik` plugin marketplace (loaded in
#     place, so edits apply on the next session or /reload-plugins), the
#     `epik@epik` plugin, and a user-scope `epik-dev` MCP server.
#   - Claude Desktop: an `epik-dev` entry in claude_desktop_config.json.
# Both `epik-dev` servers run `uv run --project <checkout>/mcp epik-mcp`, so
# they always serve the checked-out source.
#
# It never removes anything. Leftovers from older installs are reported for
# you to remove by hand.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MCP_DIR="$REPO/mcp"
SERVER_NAME="epik-dev"
DESKTOP_DIR="$HOME/Library/Application Support/Claude"
DESKTOP_CONFIG="$DESKTOP_DIR/claude_desktop_config.json"
KNOWN_MARKETPLACES="$HOME/.claude/plugins/known_marketplaces.json"

CHECK_ONLY=false
case "${1:-}" in
  "") ;;
  --check) CHECK_ONLY=true ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

section() { printf '\n== %s ==\n' "$1"; }
note() { printf '  %s\n' "$*"; }

for tool in git uv claude gh python3; do
  command -v "$tool" >/dev/null || { echo "missing required command: $tool" >&2; exit 1; }
done
UV="$(command -v uv)"

section "Checkout"
note "path:   $REPO"
note "branch: $(git -C "$REPO" rev-parse --abbrev-ref HEAD)"
note "commit: $(git -C "$REPO" describe --always --dirty)"
if gh auth status >/dev/null 2>&1; then
  note "gh:     signed in"
else
  note "gh:     NOT signed in; run: gh auth login"
fi
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  note "warning: ANTHROPIC_API_KEY is set; Claude Code will use it instead of your claude.ai login"
fi

section "Epik installs found (report only)"
note "Claude Code plugins:"
claude plugin list 2>/dev/null | grep -i -A3 'epik' | sed 's/^/    /' || note "  none"
note "Claude Code marketplaces:"
claude plugin marketplace list 2>/dev/null | grep -i -A1 'epik' | sed 's/^/    /' || note "  none"
note "Claude Code MCP servers:"
claude mcp list 2>/dev/null | grep -i 'epik' | sed 's/^/    /' || note "  none"
note "Claude Desktop MCP servers ($DESKTOP_CONFIG):"
if [ -f "$DESKTOP_CONFIG" ]; then
  python3 - "$DESKTOP_CONFIG" <<'EOF'
import json, sys
servers = json.load(open(sys.argv[1])).get("mcpServers", {})
hits = {name: cfg for name, cfg in servers.items() if "epik" in (name + json.dumps(cfg)).lower()}
for name, cfg in hits.items():
    print(f"    {name}: {cfg.get('command', '')} {' '.join(cfg.get('args', []))}")
if not hits:
    print("    none")
EOF
else
  note "  no config file"
fi
note "Claude Desktop extensions:"
if [ -d "$DESKTOP_DIR/Claude Extensions" ]; then
  ls "$DESKTOP_DIR/Claude Extensions" | grep -i 'epik' | sed 's/^/    /' || note "  none"
else
  note "  none"
fi
note "Expected after install: marketplace 'epik' from $REPO, plugin epik@epik,"
note "and '$SERVER_NAME' in Claude Code and Claude Desktop. Anything else is a leftover."
note "Until this branch becomes main, the plugin's own EpikMCP server can't start"
note "(it installs from main, which has no mcp/ yet); '$SERVER_NAME' stands in for it."

if ! $CHECK_ONLY; then
  section "Install"
  "$UV" sync --quiet --project "$MCP_DIR"
  note "mcp/ environment synced"

  existing="$(python3 - "$KNOWN_MARKETPLACES" <<'EOF'
import json, sys
try:
    entry = json.load(open(sys.argv[1])).get("epik")
except (OSError, ValueError):
    entry = None
print((entry or {}).get("installLocation", ""))
EOF
)"
  if [ -z "$existing" ]; then
    claude plugin marketplace add "$REPO"
  elif [ "$(cd "$existing" 2>/dev/null && pwd)" != "$REPO" ]; then
    echo "A marketplace named 'epik' already points at $existing." >&2
    echo "Remove it first: claude plugin marketplace remove epik" >&2
    exit 1
  else
    note "marketplace 'epik' already points at this checkout"
  fi
  claude plugin install epik@epik

  claude mcp remove "$SERVER_NAME" -s user >/dev/null 2>&1 || true
  claude mcp add -s user "$SERVER_NAME" -- "$UV" run --quiet --project "$MCP_DIR" epik-mcp
  note "Claude Code: user-scope MCP server '$SERVER_NAME' registered"

  if [ -d "$DESKTOP_DIR" ]; then
    if [ -f "$DESKTOP_CONFIG" ]; then
      backup="$DESKTOP_CONFIG.bak-$(date +%Y%m%d-%H%M%S)"
      cp "$DESKTOP_CONFIG" "$backup"
      note "Claude Desktop: config backed up to $backup"
    fi
    python3 - "$DESKTOP_CONFIG" "$SERVER_NAME" "$UV" "$MCP_DIR" "$PATH" <<'EOF'
import json, os, sys
path, name, uv, mcp_dir, shell_path = sys.argv[1:]
config = json.load(open(path)) if os.path.exists(path) else {}
config.setdefault("mcpServers", {})[name] = {
    "command": uv,
    "args": ["run", "--quiet", "--project", mcp_dir, "epik-mcp"],
    # Desktop starts servers with a minimal PATH; the server shells out to gh.
    "env": {"PATH": shell_path},
}
with open(path, "w") as f:
    json.dump(config, f, indent=2)
    f.write("\n")
EOF
    note "Claude Desktop: '$SERVER_NAME' written; quit and reopen Desktop to load it"
  else
    note "Claude Desktop: not installed; skipped"
  fi
fi

section "Smoke test: $SERVER_NAME"
python3 "$REPO/scripts/smoke_mcp.py" "$UV" run --quiet --project "$MCP_DIR" epik-mcp
