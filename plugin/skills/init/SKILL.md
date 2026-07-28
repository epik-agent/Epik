---
name: init
description: Initialize the current directory as an Epik project — per-project plugin enablement, a CLAUDE.md stub, the build-monitoring loop, a design-history scaffold, and (optionally) the GitHub repository.
disable-model-invocation: true
---

# Epik Initializes Projects

Turn the current directory into an Epik project. There is no install script:
the user installs the plugin the standard way, summons Epik here, and Epik
sets the project up itself. This skill is invoked explicitly as `/epik:init`;
when Epik is summoned (e.g. via `/epik:design`) in a directory that is not
yet an Epik project, it should *offer* to initialize and, if the user agrees,
tell them to run `/epik:init` (or the user may run it directly).

Template files referenced below live in the plugin's `templates/` directory —
the sibling of this skill's directory: this file is
`<plugin root>/skills/init/SKILL.md`, the templates are at
`<plugin root>/templates/`.

## Step 1 — Detect what's missing

Inspect the current directory and report a short checklist before touching
anything:

- `.claude/settings.json` — present? Does it already contain the `epik`
  marketplace under `extraKnownMarketplaces` and `"epik@epik": true` under
  `enabledPlugins`?
- `CLAUDE.md` — present?
- `.claude/loop.md` — present?
- `docs/design-history/` — present?
- Git — is this a git repository? Does it have an `origin` remote on GitHub?

If everything is already in place, say so and stop — there is nothing to do.

## Step 2 — Confirm

Show the user exactly what you propose to create or change (only the missing
pieces from Step 1) and ask for confirmation before writing anything. Never
overwrite or modify an existing file without asking about that file
specifically.

## Step 3 — Write the project files

With the user's confirmation, create the missing pieces:

### `.claude/settings.json` — per-project enablement

Source template: `templates/settings.json`.

- If the project has no `.claude/settings.json`, copy the template verbatim.
- If it has one, **merge — do not overwrite**: read the existing JSON, add the
  `epik` entry to `extraKnownMarketplaces` and `"epik@epik": true` to
  `enabledPlugins` (creating those objects if absent), and leave every other
  key untouched.

Once committed, any collaborator who clones the project and trusts the folder
is prompted by Claude Code to install the marketplace and plugin — the project
becomes self-installing.

### `CLAUDE.md` — project stub

Only if absent. Keep it minimal; the user will grow it:

```markdown
# <project name>

<one-line description — ask the user, or infer from the directory and confirm>

## Design

Design and theory documents live in `docs/design-history/`. Read the latest
entry before making significant design decisions.
```

### `.claude/loop.md` — build monitoring default

Copy `templates/loop.md` to `.claude/loop.md` so a bare `/loop` in this
project defaults to watching the active headless feature build. If the file
already exists, ask before replacing it.

### `docs/design-history/` — design repo scaffold

Create the directory with a single `README.md`:

```markdown
# Design history

Theory and design documents for this project, in chronological order. Each
entry is a dated Markdown file (`YYYY-MM-DD-topic.md`) capturing a design
decision, a theory of the problem, or a revision of one. Newer entries
supersede older ones; keep the old ones — the history is the point.
```

## Step 4 — GitHub repository (optional)

Only relevant when Step 1 found no GitHub `origin` remote. Ask the user
whether they want the repo created; skip this step entirely if they decline.

First check for user-level repo defaults in `~/.epik/config.json`:

```json
{
  "github": {
    "org": "my-org",
    "visibility": "private",
    "labels": [
      { "name": "epic", "color": "5319E7", "description": "Feature issue" }
    ],
    "branch_protection": true,
    "project_board": "Epik"
  }
}
```

All keys are optional:

- `org` — GitHub owner for new repos (default: the authenticated user).
- `visibility` — `"private"` or `"public"` (default: `"private"`).
- `labels` — labels to create after the repo exists (`gh label create`).
- `branch_protection` — if `true`, protect the default branch with a required
  `CI` status check (via `gh api`).
- `project_board` — name of a Project (v2) board to create and link.

Then:

1. If the directory is not a git repository, run `git init` (after
   confirming).
2. If `~/.epik/config.json` exists, create the repo with its defaults:
   `gh repo create <org>/<name> --<visibility> --source . --push` (make an
   initial commit first if needed), then apply labels, branch protection, and
   the project board as configured.
3. If the file is absent, either skip repo creation gracefully or — after
   confirming name and visibility with the user — use plain
   `gh repo create` defaults. Do not invent org, labels, or protection rules
   the user never asked for.

Note for headless feature builds: the repo also needs the `epik-build.yml`
workflow and an `ANTHROPIC_API_KEY` secret (see the plugin README). Mention
this to the user; setting secrets is theirs to do.

## Step 5 — Wrap up

Summarize what was created. Suggest committing the new files (ask before
running `git commit`). Health checks (`doctor`) are a future feature — this
skill only initializes.
