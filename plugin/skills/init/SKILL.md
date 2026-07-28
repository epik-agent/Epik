---
name: init
description: Converge this project on the correct Epik shape — create the repository, convert an existing one, or repair drift (auth, enablement, the build workflow, secrets, conventions) through a detect → offer → fix → report dialogue. Idempotent; safe to run any number of times.
disable-model-invocation: true
---

Read `init.md` in this skill's directory and follow it.

(That file is the canonical spec of a correct Epik project — creation and
repair are both "apply the spec," differing only in how much of it is missing.
The EpikMCP server vendors it and serves it as the `init-epik` MCP prompt, so
the same dialogue works on surfaces that don't load plugins — keep it
self-contained.)

This skill is invoked explicitly as `/epik:init`. Typing it is the convergence
gate applied to projects: the deliberate act that says "this is now an Epik
project." When Epik is summoned elsewhere (e.g. via `/epik:design`) and notices
the project isn't ready, it should *offer* to converge and point the user here
rather than reaching for this skill itself.
