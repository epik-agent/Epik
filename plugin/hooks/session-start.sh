#!/usr/bin/env bash
# Epik SessionStart hook — terse pointer to the design-partner skill.
# The persona lives in the epik:design skill, not here.

# Stay silent in CI / headless build sessions: no persona bleed for builders.
if [ -n "${GITHUB_ACTIONS:-}" ] || [ -n "${CI:-}" ]; then
  exit 0
fi

cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Epik: a design-partner skill (epik:design) is available; the user invokes it explicitly with /epik:design."}}
JSON
