#!/usr/bin/env bash
# Epik SessionStart hook — terse pointer to the summoning skill.
# The persona lives in the epik:summon skill, not here.

# Stay silent in CI / headless build sessions: no persona bleed for builders.
if [ -n "${GITHUB_ACTIONS:-}" ] || [ -n "${CI:-}" ]; then
  exit 0
fi

cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Epik: a software-design partner (epik:summon) is available; the user summons it explicitly with /epik:summon."}}
JSON
