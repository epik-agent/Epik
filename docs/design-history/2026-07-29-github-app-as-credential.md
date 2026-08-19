# GitHub App as credential, not service

- Status: Proposed
- Date: 2026-07-29
- Note: v3, reconstructed 2026-07-31 — the v2.1 file delivered by chat drop
  was lost before check-in. Content rebuilt from session records; the
  decisions are as discussed on 07-29/07-30.

## Context

The MCPB documentation notes that "remote MCP servers are recommended" for
directory listing. Read carelessly, that reopens the
[presence-not-a-connector](2026-07-28-presence-not-a-connector.md) decision
against hosted infrastructure. It does not: the note is about
discoverability in a directory, not capability, and locally-run MCPB
bundles remain directory-submittable. All of the presence ADR's reasons
stand.

But the discussion untangled something more useful: **a GitHub App needs
no server at all.** Webhooks and OAuth login are *optional* App
capabilities; GitHub Actions is already Epik's event receiver. Stripped to
its core, a GitHub App is an app ID plus a private key — a pure
credential with an identity attached.

### The two canonical user scenarios

These are the requirements bar for any auth/identity design:

1. **Single user.** Epik is a distinct persona — it presents like a
   separate collaborator without being one — easily available: summoned,
   or already attached to a repository. The user may access multiple
   GitHub organizations.
2. **Organization.** The org "hires an employee named Epik": available to
   every member, summonable by all, possibly policy-attached to every
   repository in the org.

In both: a clearly defined Epik persona. "Hello, I'm Epik." An avatar
wherever possible. Epik's commits attributed to Epik; humans still commit
as themselves.

(Terminology, deliberate: Epik is a **persona**, never a "person." A
persona is not a person but nevertheless has the identity and sense of
unified agency that a person does. Similes — "like a separate person,"
"as if the company hired an employee" — are fine; the direct noun is
not.)

## Decision

**Adopt a GitHub App as Epik's credential and identity for GitHub Actions
builds, decoupled from any remote MCP server.** No webhooks, no OAuth
flow, no hosted anything.

### The unit is the trust domain

The App's boundary is *who holds the private key* — not per-user, not
per-repo.

- **Scenario 1** = one App, registered by the operator, installed across
  all of the operator's organizations.
- **Scenario 2** = still a single trust domain: the org registers its own
  App under the org account. A bigger single tenant — **not
  multi-tenancy.** The "attached to every repo" policy is the App's
  "All repositories" installation scope, which covers future repos
  automatically; per-repo workflow files remain `init`'s job.
- **Independent adopters** get per-domain App registrations (via the
  manifest flow below). Private keys are never shared across trust
  domains.

Only one human per trust domain ever registers an App; org members do
nothing App-side — their per-seat step is installing the plugin/MCPB.

### Naming

`epik-acme[bot]` is fine. Not every bot needs to be literally named
"epik"; the persona lives in the avatar and the behavior, not the slug.
This removes uniform naming as a driver for any centralized design.

### Identity split on intent

The simple split, chosen deliberately ("go with the easy thing for now"):

- **Chat-driven writes** — the user, via their local `gh`. The MCPB is
  untouched by this ADR.
- **Automation writes** (headless builds) — `epik[bot]`, via the App.

Recorded but not chosen: App user-to-server tokens via the device flow
(still serverless) would render chat-driven writes as "user, via Epik"
with an app badge — appropriate if asking Epik should ever visibly be
Epik's act. Explicitly undecided.

### Commit attribution is not automatic

The scenarios require "Epik's commits attributed to Epik," and pushes
authenticated by an installation token do **not** produce that by
themselves. Workflows must set the git author to
`{slug}[bot]` / `{app-id}+{slug}[bot]@users.noreply.github.com` — the
format GitHub links to the App's avatar. Otherwise pushes succeed and the
requirement silently fails. This is an explicit action item and an init
check.

### What the App cannot deliver

"Easily summoned by every org member" is the one Scenario-2 requirement
outside the App's reach: the persona travels in the plugin/MCPB, per
seat. Org-wide client distribution (Claude Team/Enterprise extension
allowlists) is a real path but outside this ADR.

### Installation tokens expire hourly — wrinkle, not wall

Builds mint on demand: JWT → installation token, cached, re-minted as
needed, exposed as a git credential helper and `GH_TOKEN` source. New
failure mode for init/diagnostic checks: silent token expiry mid-push in
builds longer than an hour; a re-mint test on a >1h build is an action
item.

### The secrets contract is the sequencing interface

`EPIK_APP_ID` and `EPIK_APP_PRIVATE_KEY`, as per-org secrets. Everything
that *consumes* the App (workflow mint helper, git author config, init
checks) is built once, now, against a manually registered App. Everything
that *provisions* the App can come later without touching the
consumption layer.

### Provisioning: two clicks and one paste, zero infrastructure

When automation is wanted, the manifest flow needs no server:

1. A static, auto-submitting form on epik-agent.dev launches the GitHub
   App-manifest flow.
2. GitHub redirects to a static page with `?code=` in the URL; the user
   pastes the code into init.
3. Init exchanges it at `POST /app-manifests/{code}/conversions` (no
   auth beyond the code), receives the pem / app ID / slug, sets the
   secrets, and emits the install link
   `github.com/apps/{slug}/installations/new`; the App's `setup_url`
   bounces to a static "done" page.

A localhost listener (zero paste) is polish. This path is cheap enough
that form and exchange may share one issue — filed, not built.

## Escalation stations (recorded, not taken)

- **Central App + hosted token service** (the Octo-STS pattern): only if
  zero-step provisioning at scale ever matters. Uniform naming is no
  longer a driver.
- **Remote MCP + App OAuth**: only if the presence ADR's reopening
  condition fires — a real (not aesthetic) mobile/web need that the
  stock GitHub connector plus conventions cannot meet.

Honest tension, on the record: Cowork-cloud sessions' reliance on the
desktop bridge is the strongest live evidence *for* a remote server. But
App-authenticated `workflow_dispatch` entry points likely close the
write gap serverless, so the tension does not yet justify the station.

## Action items

1. Manual App registration under the epik-agent org (webhooks off;
   permissions: contents/issues/PRs/workflows read-write, actions read;
   install scope "All repositories").
2. Set the `EPIK_APP_ID` / `EPIK_APP_PRIVATE_KEY` org secrets.
3. Mint-on-demand helper in `epik-build.yml`; retire PATs from the build
   path, including `EPIK_BUILD_GH_TOKEN`'s role.
4. Git author configuration in workflows (attribution requirement).
5. Init checks: secrets present, token mintable, attribution configured.
6. Re-mint test on a build exceeding one hour.
7. File (do not build) the manifest-flow provisioning issue.
