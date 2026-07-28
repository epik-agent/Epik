# Templates

Files that Epik writes into *other* projects. Nothing in this directory affects the Epik plugin itself.

## `settings.json`

The per-project enablement template. The `epik:init` skill writes it to a project's `.claude/settings.json` (merging with any existing settings, not overwriting) to make the project self-installing for collaborators:

- `extraKnownMarketplaces` registers the [`epik-agent/Epik`](https://github.com/epik-agent/Epik) GitHub repository as a plugin marketplace named `epik`.
- `enabledPlugins` enables `epik@epik` (the `epik` plugin from the `epik` marketplace — the names come from this repo's [`.claude-plugin/marketplace.json`](../../.claude-plugin/marketplace.json)).

Once the file is committed, any collaborator who clones the project and trusts the folder is prompted by Claude Code to install the marketplace and plugin — no install script, no manual `/plugin` commands. See [`/epik:init` — idempotent convergence](../../README.md#epikinit--idempotent-convergence) in the root README for the full story.
