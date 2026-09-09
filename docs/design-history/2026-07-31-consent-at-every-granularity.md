# Consent at every granularity

- Status: Proposed
- Date: 2026-07-31

## Context

Three consent moments have accreted in Epik's design, each decided
separately:

1. **Presence.** Epik is summoned explicitly; the summons is the user's
   act ("a doorbell is not a door that opens itself" —
   [presence-not-a-connector](2026-07-28-presence-not-a-connector.md)).
2. **Enrollment.** A project becomes an Epik project when the user types
   `/epik:init-repository`; "the typed command is the consent"
   ([init-is-idempotent-convergence](2026-07-28-init-is-idempotent-convergence.md)).
3. **Action.** During install testing (2026-07-31), the operator's client
   was set to require approval for every tool action. The first instinct
   was to treat this as friction — and the failure mode it produced was
   real: from a cloud session, a pending approval prompt is
   indistinguishable from a crashed server (a silent multi-minute timeout
   blamed on the tool).

The question was whether to engineer the third moment away — recommend
permissive settings, seek blanket always-allow — or to recognize it as
the same posture the first two already express.

## Decision

Name the principle: **Epik never acts without the user's say-so, at any
granularity.** Presence is consented by summoning, enrollment by the
typed init command, and individual actions by the client's approval
prompts. These are one posture, not three frictions.

Consequently:

- Epik **endorses manual action approval as the intended posture** for a
  tool that acts on GitHub under the user's identity. Documentation
  states this proudly rather than apologizing for it.
- Epik **never recommends blanket always-allow** on broad tools.
- The persona **narrates the handshake**: on fresh installs it orients
  the user that approval prompts are expected, and it announces its
  first action so the first dialog reads as intended behavior rather
  than a mystery (#76).
- Documentation carries the troubleshooting corollary: a hung-seeming
  chat usually means an approval prompt is waiting on the user's screen
  (#74, #75, #76 touch the same files).
- **Future features must locate their consent moment at design time.**
  A capability whose consent story cannot be stated in one sentence is
  not ready.

## Consequences

Host permission layers (client approval settings, platform action
classifiers) become part of Epik's design surface rather than obstacles
to route around. The cost is clicks; the payoff is that Epik's safety
story is uniform from the first hello to the last push.

Open question, deliberately not decided here: whether the breadth of the
`gh_raw` escape hatch should itself be revisited, since a broad tool
invites broad always-allow grants. That belongs with the multi-org auth
story.
