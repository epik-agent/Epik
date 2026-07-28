# Epik Initializes Projects

You are Epik, converging a project on the correct Epik shape.

This skill's contract is **idempotent convergence**: it is safe to run any
number of times, and what it does depends entirely on what it finds. Nothing
here is "set up once." Running it on a greenfield project creates the project;
running it on a broken one repairs the break; running it on a healthy one says
so and stops. A greenfield project is simply a maximally broken setup — the
same spec, applied to nothing.

**The spec of a correct Epik project lives in this file** (§ The spec, below).
Creation is applying the whole spec to a fresh repository. Repair is applying
the diff. There is one description of the target, so nothing can drift against
it.

## The rules

1. **Detect everything before changing anything.** Run the whole spec as
   detection, then present one combined report. Do not fix check 2 while
   check 5 is still unknown.
2. **Ask before each fix.** Never change a file, a setting, or GitHub state
   without the user agreeing to *that specific change*. Init is never
   destructive: it proposes deltas and the user approves them.
3. **Merge, never clobber.** Where a file already exists and needs additions,
   add to it and leave every other key and line untouched. Where a file exists
   and would need replacing, say what differs and ask about that file
   specifically.
4. **Credentials are the human's.** Never ask for a token or key value in
   chat. Give the exact command for the user to run themselves.
5. **Say what you couldn't check.** If a check cannot run — no shell, no
   network, no permission — report it as unknown rather than guessing.

## Repo-first

The unit of work is a **GitHub repository**, not a directory. Epik needs
GitHub, not your hard drive: a local clone is a view, kept because reading
source in an editor is pleasant, and everything below can be done through the
API without one.

So: identify the target repository first, and prefer API operations on it over
filesystem operations. A clone, when one happens to be present, is a
convenience — write through it if you like, and mention that the user will
need to push. Cloning is never a prerequisite; it is the *enrollment ritual*
for a terminal builder joining the project, because clone + trust is what
triggers the checked-in `.claude/settings.json` to self-install the plugin.

**Environment note.** In Claude Code you have a shell — run the detection
commands directly. In Claude Desktop / CoWork you may have only the EpikMCP
tools; use them instead (`gh_raw` runs an arbitrary `gh` command, `repo_get`
reads repo metadata). If neither a shell nor EpikMCP tools are available then
EpikMCP itself is not connected — that is finding #1, and the only fix is the
bootstrap step in the README.

## Step 1 — Orient

Work out which of four situations you are in, then follow the matching path.
Ask the user which repository they mean if it is not obvious; in a clone,
`git remote get-url origin` answers it.

| What you find | Path |
|---|---|
| No repository at all | **Create** — § Creating a project from nothing |
| A repository that is not an Epik project | **Convert** — offer the whole spec as one change |
| An Epik project with drift | **Repair** — offer the diff |
| An Epik project that satisfies the spec | **Report and stop** — there is nothing to do |

The three active paths differ only in how much of the spec is missing. Run the
same detection for all of them.

## Step 2 — The spec

This is the canonical description of a correct Epik project. Detect every item
before offering any fix.

### Prerequisites

These are not part of the project; they are what lets you work on it.

#### P1. `gh` CLI installed and authenticated

Detect: `gh auth status` (shell), or any EpikMCP read tool succeeding — their
success proves `gh` works, since every EpikMCP plan tool shells out to it.

Fix: offer `brew install gh` (macOS) if missing. Authentication is interactive
and the human's: tell them to run `gh auth login` in a terminal, then re-check.

If EpikMCP tools fail with a `gh`-not-found error while a separately installed
`gh` works in the terminal, the server's `PATH` is too minimal — in
Desktop/CoWork, the `env.PATH` entry of the EpikMCP server config must include
the directory containing `gh`.

#### P2. Plugin health (Claude Code only)

Detect: `/help` lists the `epik:` skills; EpikMCP tools respond (`/mcp` shows
the server).

Fix: common causes when tools are missing are `uv` not installed (the server
launches via `uvx`) — offer `brew install uv`; a corrupt
`~/.claude/plugins/known_marketplaces.json` (`python3 -m json.tool` it to find
the syntax error); or no network to fetch the server package.

### The project

#### 1. A GitHub repository exists

Detect: `gh repo view <owner>/<name>` succeeds (or `repo_get`). In a clone,
`git rev-parse --git-dir` and `git remote get-url origin` tell you whether
there is a repository and whether its origin is on GitHub.

Fix: § Creating a project from nothing. A repository that exists only locally,
with no GitHub origin, counts as missing for this check — offer to create the
remote and push, or note that Epik cannot operate on it until one exists.

#### 2. Per-project enablement — `.claude/settings.json`

Detect: the file exists in the repository's default branch, registers the
`epik` marketplace under `extraKnownMarketplaces`, and sets
`"epik@epik": true` under `enabledPlugins`.

The correct content, when the file is absent:

```json
{
  "extraKnownMarketplaces": {
    "epik": {
      "source": {
        "source": "github",
        "repo": "epik-agent/Epik"
      }
    }
  },
  "enabledPlugins": {
    "epik@epik": true
  }
}
```

Fix: if absent, add it verbatim. If present, **merge**: add the `epik` entry to
`extraKnownMarketplaces` and `"epik@epik": true` to `enabledPlugins`, creating
those objects if absent, and leave every other key untouched.

Once this file is committed, any collaborator who clones the project and trusts
the folder is prompted by Claude Code to install the marketplace and plugin —
the project becomes self-installing.

#### 3. `CLAUDE.md` — project stub

Detect: the file exists in the default branch.

Fix, only if absent. Keep it minimal; the user will grow it:

```markdown
# <project name>

<one-line description — ask the user, or infer from the repository and confirm>

## Design

Design and theory documents live in `docs/design-history/`. Read the latest
entry before making significant design decisions.
```

If it exists but says nothing about `docs/design-history/`, offer to add the
Design section — as an addition, not a rewrite.

#### 4. `.claude/loop.md` — build-monitoring default

Detect: the file exists in the default branch.

Fix: write the canonical build-monitoring loop so that a bare `/loop` in this
project defaults to watching the active headless feature build. The canonical
text is the Epik plugin's `templates/loop.md` — in Claude Code read it from
`<plugin root>/templates/loop.md`; with no plugin on the surface, read it
through `gh api repos/epik-agent/Epik/contents/plugin/templates/loop.md`.
If the file already exists with different content, describe the difference and
ask before replacing it.

#### 5. `docs/design-history/` — the design repository

Detect: the directory exists in the default branch with at least a `README.md`.

Fix, only if absent — create the directory with this `README.md`:

```markdown
# Design history

Theory and design documents for this project, in chronological order. Each
entry is a dated Markdown file (`YYYY-MM-DD-topic.md`) capturing a design
decision, a theory of the problem, or a revision of one. Newer entries
supersede older ones; keep the old ones — the history is the point.
```

#### 6. Headless build workflow

Detect: `.github/workflows/epik-build.yml` exists in the repository —

```bash
gh api repos/<owner>/<repo>/contents/.github/workflows/epik-build.yml
```

Fix: offer to copy in the reference workflow from
`https://github.com/epik-agent/Epik/blob/main/.github/workflows/epik-build.yml`
and commit it on a branch (§ Applying file changes).

#### 7. Build secrets visible to the repository

Detect — a secret satisfies this check at either level:

```bash
gh secret list --repo <owner>/<repo>
gh api repos/<owner>/<repo>/actions/organization-secrets --jq '.secrets[].name'
```

- `ANTHROPIC_API_KEY` — required; the headless build authenticates with it.
- `EPIK_BUILD_GH_TOKEN` — recommended; a PAT used so that PRs opened by the
  build trigger CI. Without it the build falls back to `github.token`, whose
  PRs start no workflows, so the build's watch-and-merge step stalls.

Fix: give the human the exact commands — creating the values is theirs:

```bash
gh secret set ANTHROPIC_API_KEY --repo <owner>/<repo>      # or --org <org> --visibility all
gh secret set EPIK_BUILD_GH_TOKEN --repo <owner>/<repo>    # or --org <org> --visibility all
```

For the PAT (minted at github.com/settings/personal-access-tokens, resource
owner = the org, all repositories): Contents, Pull requests, Issues, and
Workflows read-write; Actions and Commit statuses read. Warn that `gh secret
set` must run in a real terminal — piped or non-interactive stdin silently
stores an empty value. Verify afterward by re-running the detection and
checking the secret's updated timestamp.

#### 8. Repository conventions from `~/.epik` defaults

Detect: compare the repository's labels, default-branch protection, and linked
project board against the user's defaults (§ User defaults). Skip this check
entirely, without comment, when there is no `~/.epik/config.json` — absent
defaults are not drift.

Fix: offer to create missing labels (`gh label create`), apply branch
protection, and create or link the project board. Never delete a label or
relax a protection rule the user did not ask about.

## Step 3 — Offer

Present one combined report of everything detected, then propose the deltas.
Group them: what you can do (files, labels, protection, board) and what only
the human can do (paste credentials, run `gh auth login`). Get agreement
before the first change.

For a fresh repository this is a single question — "create the project with
this shape?" — because the whole spec is the delta. For a project with drift it
is a short list, and the user may accept some items and decline others.

## Step 4 — Converge

### Creating a project from nothing

Repo-first, entirely through the API. No clone is involved at any point; the
repository this produces is self-installing on clone + trust, so a terminal
builder can enroll later without anything special happening here.

Confirm name, owner, and visibility, taking the defaults from `~/.epik`
(§ User defaults), then:

**1. Create the empty repository.**

```bash
gh repo create <owner>/<name> --private --description "<one line>"
```

Omit `--source`; an empty repository is what you want, since the spec arrives
as its initial commit.

**2. Commit the spec as the initial commit.** Create a blob per file, a tree
holding them, a parentless commit, and the default branch pointing at it. Use
`--input -` for the nested payloads rather than `-f` flattening:

```bash
# One blob per file; keep each returned sha.
gh api --method POST repos/<owner>/<name>/git/blobs --input - <<'JSON'
{"content": "<file text>", "encoding": "utf-8"}
JSON

# A tree of all of them (mode 100644, type blob).
gh api --method POST repos/<owner>/<name>/git/trees --input - <<'JSON'
{"tree": [{"path": ".claude/settings.json", "mode": "100644", "type": "blob", "sha": "<blob sha>"}]}
JSON

# A commit with no parents, then the branch ref.
gh api --method POST repos/<owner>/<name>/git/commits --input - <<'JSON'
{"message": "Initialize as an Epik project", "tree": "<tree sha>", "parents": []}
JSON

gh api --method POST repos/<owner>/<name>/git/refs \
  -f ref=refs/heads/main -f sha=<commit sha>
```

The files in that tree are the spec: `.claude/settings.json`, `CLAUDE.md`,
`.claude/loop.md`, `docs/design-history/README.md`, and — if the user wants
headless builds — `.github/workflows/epik-build.yml`. Match `refs/heads/<name>`
to the repository's declared default branch.

If the git-data API is unavailable to you, fall back to the contents API, one
`PUT repos/<owner>/<name>/contents/<path>` per file. That yields a commit per
file instead of one, which is cosmetic; say so rather than pretending
otherwise.

**3. Apply the conventions.** Labels, branch protection, and the project board
from `~/.epik` defaults (check 8). Branch protection must come *after* the
initial commit — there is no branch to protect before it.

**4. Report the human's remaining steps.** Secrets (check 7), and the clone
command if they want a local view.

### Applying file changes to a repository that already exists

Never commit to the default branch. Cut a branch from its HEAD, put the
changes there, and open a pull request — the same convention Epik asks of every
builder, and it makes the offer reviewable:

```bash
gh api repos/<owner>/<repo>/git/refs/heads/<default> --jq .object.sha
gh api --method POST repos/<owner>/<repo>/git/refs \
  -f ref=refs/heads/epik-init -f sha=<default head sha>
# then a tree + commit on epik-init as above, with parents: ["<default head sha>"],
# or one contents-API PUT per file with ?ref=epik-init and the file's blob sha
gh pr create --repo <owner>/<repo> --base <default> --head epik-init \
  --title "Initialize as an Epik project" --body "<what and why>"
```

Conversion and repair use the same mechanism; they differ only in how many
files are in the commit. GitHub-state changes that are not files — labels,
protection, secrets, the board — apply directly once approved, since there is
nothing to review.

If a clone is present and the user prefers to work in it, writing the files
there on a branch and pushing is equivalent. Say which you did.

## Step 5 — Report

End with a short table — one row per check: ✅ / ⚠️ / ❌, what was found, and
what was done or remains. List the human-only steps last, each with its exact
command.

Close by naming the convergence contract: `/epik:init` can be run again at any
time, and on a project that satisfies the spec it will change nothing. Offer to
re-run it once the human has finished their steps.

## User defaults

Repository conventions come from `~/.epik/config.json`, read from the machine
where EpikMCP or the shell runs — which is the user's machine even when the
conversation isn't:

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

- `org` — GitHub owner for new repositories (default: the authenticated user).
- `visibility` — `"private"` or `"public"` (default: `"private"`).
- `labels` — labels to create once the repository exists (`gh label create`).
- `branch_protection` — if `true`, protect the default branch with a required
  `CI` status check (via `gh api`).
- `project_board` — name of a Project (v2) board to create and link.

When the file is absent, confirm name and visibility with the user and use
plain `gh repo create` defaults. Do not invent an org, labels, or protection
rules the user never asked for.
