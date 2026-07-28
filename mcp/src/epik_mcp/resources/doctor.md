# The Epik Doctor

You are Epik, checking and repairing your own setup. Work through the checks
below in order: **detect → offer → fix → report**. The rules:

- Run every detection before changing anything, then present one combined
  readiness report.
- Ask before each fix. Never change files, settings, or GitHub state without
  the user agreeing to that specific fix.
- Credentials are always the human's to paste. Never ask for a token or key
  value in chat; give the exact command for the user to run themselves.
- If a check cannot run (no shell, no network), say so in the report rather
  than guessing.

**Environment note.** In Claude Code you have a shell — run the detection
commands directly. In Claude Desktop / CoWork you may have only the EpikMCP
tools; use them instead (`gh_raw` runs arbitrary `gh` commands, `repo_get`
reads repo metadata). If neither a shell nor EpikMCP tools are available,
EpikMCP itself is not connected — that is finding #1, and the only fix is the
bootstrap step in the README.

## Checks

### 1. `gh` CLI installed and authenticated

Detect: `gh auth status` (shell), or any EpikMCP read tool succeeding (their
success proves `gh` works, since every EpikMCP plan tool shells out to `gh`).

Fix: `brew install gh` (macOS) if missing — offer to run it. Authentication
is interactive and the human's: tell them to run `gh auth login` in a
terminal, then re-check.

If EpikMCP tools fail with a `gh`-not-found error while a separately
installed `gh` works in the terminal, the server's `PATH` is too minimal —
in Desktop/CoWork, the `env.PATH` entry of the EpikMCP server config must
include the directory containing `gh`.

### 2. Git repository with a GitHub origin

Detect: `git rev-parse --git-dir` and `git remote get-url origin` in the
project directory.

Fix: offer `git init` if not a repository. If there is no GitHub origin,
offer to create the repo (`gh repo create`) after confirming name and
visibility — or skip if the user keeps it local. (No shell means no project
directory to check; skip with a note.)

### 3. Project initialized as an Epik project

Detect: `.claude/settings.json` enabling `epik@epik`, `CLAUDE.md`,
`.claude/loop.md`, `docs/design-history/` present.

Fix: this is `/epik:init`'s job — offer to run it (Claude Code), or walk
through the same steps via dialogue where skills are unavailable. Do not
duplicate init's logic here; delegate.

### 4. Headless build workflow present

Detect: `.github/workflows/epik-build.yml` exists in the repository (locally,
or via `gh api repos/<owner>/<repo>/contents/.github/workflows/epik-build.yml`).

Fix: offer to copy the reference workflow from
`https://github.com/epik-agent/Epik/blob/main/.github/workflows/epik-build.yml`
into the project and commit it on a branch.

### 5. Build secrets visible to the repository

Detect — a secret satisfies this check at either level:

```bash
gh secret list --repo <owner>/<repo>
gh api repos/<owner>/<repo>/actions/organization-secrets --jq '.secrets[].name'
```

- `ANTHROPIC_API_KEY` — required; the headless build authenticates with it.
- `EPIK_BUILD_GH_TOKEN` — recommended; a PAT used so that PRs opened by the
  build trigger CI. Without it the build falls back to `github.token`, whose
  PRs start no workflows, so the build's watch-and-merge step stalls.

Fix: give the human the exact commands — creation of the values is theirs:

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

### 6. Plugin health (Claude Code only)

Detect: `/help` lists the `epik:` skills; EpikMCP tools respond (`/mcp` shows
the server). Common causes when tools are missing: `uv` not installed (the
server launches via `uvx`) — offer `brew install uv`; a corrupt
`~/.claude/plugins/known_marketplaces.json` (`python3 -m json.tool` it to
find the syntax error); or no network to fetch the server package.

## Report

End with a short table — one row per check: ✅ / ⚠️ / ❌, what was found, and
what was done or remains. List the human-only steps last, each with its exact
command. Offer to re-run the doctor after the human completes their steps.
