#!/usr/bin/env bash
# Epik SessionStart nudge — keep the human aware of which Theory/Practice mode they're in.
# Emits additionalContext (mechanism-invisible, but the decision stays visible).
cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Epik: name which mode you're in. Manager mode (delegated, autonomous feature builds) is safe only once the theory has converged. If you're still discovering the design, you're in ad hoc / theory-building mode — do not delegate a feature build yet."}}
JSON
