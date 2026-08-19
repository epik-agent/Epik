# ADR: Presence, not a connector

- **Status:** Accepted
- **Date:** 2026-07-28
- **Source:** Design conversation (Cowork), July 2026

## Context

Three desires surfaced in quick succession during design sessions: Epik should
be a claude.ai connector (with the little icon), Epik should be white-labeled,
and — the telling one — every session opens with the question "Is this Epik?"

These are one need wearing three costumes: **ambient presence indication**.
There is currently no affordance that tells the user whether the Epik persona
and machinery are loaded in a given session. The user must probe. GitHub's
connector icon answers "is the connector here?" at a glance; Epik answers it
only under interrogation.

The friction is partly self-inflicted. The "Epik is summoned explicitly"
decision (`disable-model-invocation` on the design skill, SessionStart hook
reworked to a thin pointer, silent in CI) was implemented as *total silence*.
But presence-announcement and auto-invocation are different things that got
collapsed together. A doorbell is not a door that opens itself: Epik can
refuse to act until summoned while still saying "I'm here."

Standing constraints that shape the answer:

- **No owned infrastructure.** The engine is GitHub Actions; GitHub is the
  dashboard; there is no chat UI and no hosted service. Epik leans entirely
  on GitHub.
- **Epik is more than tools.** Its identity lives in skills (design persona,
  init, feature), the loop.md monitoring convention, the SessionStart hook,
  and the checked-in settings that make projects self-installing.
- **Epik's real interface is GitHub state** — issues, labels, project status,
  Actions runs. Any GitHub client is already, in effect, an Epik client.

## Decision

Epik will **not** become a hosted connector. The presence need is met
in-band, on the surfaces Epik already occupies:

1. **SessionStart banner (terminal).** The hook stays silent in CI but, in
   interactive sessions, emits a one-line identity banner — name, tagline,
   how to summon. Presence without invocation.
2. **Statusline marker (Claude Code).** A persistent "Epik" indicator in the
   Claude Code statusline for Epik projects. This is the closest analogue to
   the coveted connector icon: always visible, glanceable, ambient.
3. **Cowork/claude.ai greeting.** The attached Project's instructions direct
   Claude to open sessions by identifying as Epik, converting silent context
   into a visible greeting. The `summon-epik` MCP prompt remains the explicit
   summon on plugin-less surfaces.
4. **Rename the summon: `/epik:design` → `/epik:hello`.** The summon word is
   a doorbell, not a feature; "hello" names the act, and the response writes
   itself: "Hello. I'm Epik." Functional skills (`feature`, `issue`, `init`)
   keep functional names; the skill's `description` field still says plainly
   that this summons the design partner, so listings remain discoverable.

   *Amended 2026-07-29 ([#55](https://github.com/epik-agent/Epik/issues/55)):
   `/epik:hello` was superseded by `/epik:summon` before it shipped — the
   summoning verb is **summon**, the word the `summon-epik` MCP prompt already
   used, so one rename served both surfaces.*

## Options Considered

### Option A: Hosted connector (remote MCP server)

| Dimension | Assessment |
|-----------|------------|
| Complexity | High — OAuth app, token custody, multi-tenancy, uptime |
| Cost | Ongoing hosting + security surface |
| Reach | Web and mobile chat, no local install |
| Fit with doctrine | Contradicts the no-servers principle |

**Pros:** the icon; availability on web/mobile without the desktop app or a
local `gh`; reach to non-Claude-Code users.

**Cons:** Epik's first owned infrastructure, existing mostly to proxy GitHub
calls; carries only tools — the skills, hooks, persona, and conventions that
constitute Epik's identity cannot ship through a connector; duplicates reach
the stock GitHub connector already provides for reads (and plausibly for
launches, if launching stays "create an issue / dispatch a workflow").

### Option B: In-band presence (accepted)

| Dimension | Assessment |
|-----------|------------|
| Complexity | Low — hook output, statusline config, a settings line, a rename |
| Cost | None ongoing |
| Reach | The surfaces Epik already targets |
| Fit with doctrine | Consistent — zero servers |

**Pros:** solves the actual friction ("Is this Epik?") everywhere it occurs;
cheap; reversible; strengthens rather than dilutes the explicit-summon model.

**Cons:** does nothing for phone/web reach; the icon envy is sublimated, not
satisfied.

### Option C: Status quo (total silence)

Rejected. The recurring "Is this Epik?" probe is a measured cost, paid every
session, for a distinction (announcement vs. invocation) that turned out to
be a conflation.

## Trade-off Analysis

The connector's genuine value is reach; everything else about it is an icon.
Reach is a *convention* problem before it is a hosting problem: because
Epik's interface is GitHub state, the stock GitHub connector on every chat
surface can already read Epik's world, and can launch work to the extent the
launch protocol is dumb enough (label conventions, workflow dispatch). Taking
on a server to duplicate that would spend Epik's cheapest asset — having no
infrastructure — to buy legibility that branding-shaped work delivers for
free.

## Consequences

- Every interactive session in an Epik project announces itself; the
  "Is this Epik?" probe retires.
- White-labeling pressure is redirected inward: brand the inside of sessions
  (banner, greeting, statusline), never the surface itself.
- The rename touches: the skill directory (`plugin/skills/design/` →
  `plugin/skills/summon/`, keeping the persona file), the hook's pointer text,
  the README summoning table, and possibly the `summon-epik` MCP prompt name
  (left unchanged — it was already the right word).
- Phone/web reach remains unserved by design. **Reopening condition:** if
  mobile/web access becomes a real requirement (not an aesthetic one), the
  hosted-EpikMCP V2 item reopens — evaluated first against "can the stock
  GitHub connector plus conventions do this?"

## Action Items

1. [ ] Rework the SessionStart hook: silent in CI, one-line banner interactively.
2. [ ] Add an Epik statusline marker for Claude Code sessions in Epik projects.
3. [ ] Add the identify-as-Epik line to the Claude Project instructions.
4. [x] Rename the summon; update hook pointer, READMEs, and the summoning
       table; decide whether `summon-epik` changes name. Shipped as
       `/epik:summon` (not `/epik:hello`); the MCP prompt names are unchanged.
5. [ ] Record the reopening condition on the V2 hosted-EpikMCP item.
