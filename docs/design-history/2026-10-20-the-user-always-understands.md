# ADR: The user always understands what is going on

- **Status:** Accepted
- **Date:** 2026-10-20
- **Source:** Design conversation (Cowork), October 2026

## Context

`crates/epik-core/prompt.md` is the assistant's system prompt, and it is one
line long. It is where Epik states how it believes software should be built;
the briefs beside it (`issue-brief.md`, `fix-resumed.md`, `fix-fresh.md`) are
the standing part of what a launched agent is told, and they are mechanical.

The assistant plans with the user. The agents that write code never see
`prompt.md`: an agent knows what its brief and its issue tell it, and what it
reads in the repository. So a principle reaches code by one of two routes —
as a decision the assistant has already made and written into an issue, or as
a rule in the agent's own brief.

The failure this entry is written against is the common one in AI-written
code: the code arrives faster than the person who owns it can follow, and
they stop understanding their own codebase.

## Decisions

### Understanding is the central principle

**The user always understands what is going on** — in the codebase, and in
the build that is changing it. The test is surprise: the user can say what
any part of the codebase does and why it is there, and nothing a build did
would surprise them on reading it.

Most of what follows is this principle applied. Source code is a liability
because every line is something the user has to hold. Boring code, accurate
names and clear architecture are what make holding it cheap. Deferring
features keeps the amount small. Rolling back is cheap for an agent and costs
the user attention, so it is done deliberately.

Rejected: throughput as the measure — generate freely and let the user catch
up, or not. It is faster until the first change nobody can explain.

### Shipping is the goal

Every decision serves a release the user can use. Understanding is the
constraint the goal is pursued under, never traded for it; within it, scope
is cut to ship.

### Three invariants, never traded

1. **The tests are always green, and every feature ends with a working
   application.** Every commit builds and passes its tests. A completed
   feature leaves the application working and usable; the issues on the way
   there need not, and a feature's issues may form any dependency graph.
   Work oscillates: write the smallest code that tests an idea, run it,
   revise. Breaking this is fixed before anything else.
2. **Every change can be explained in a few sentences.** One that cannot is
   too large or too clever, and is split or simplified. The obvious
   construction over the elegant one. Things are named for what they are and
   renamed when that changes. Someone who knows the project can guess where
   any code lives.
3. **Every behavior has a test, written first.** The test fails, then the
   code makes it pass. Coverage measures this and is not its goal — the
   reading `codecov.yml` already states, a number that gates gets gamed.

### Three dials, turned deliberately and out loud

4. **Speed against debt.** Debt is taken on when it reaches a release
   sooner, said when it is taken, and recorded where it will be found.
5. **Scope.** In doubt, leave it out. A feature is built when it is needed,
   not when it is foreseen. Cutting a feature to ship is a success.
6. **Novelty.** Decide the one thing the program does differently. For
   everything else, use an existing crate, pattern or convention, even where
   a bespoke one would be marginally better.

### Four habits

7. **Write reluctantly.** Before adding code, ask whether deleting code,
   configuration, or an existing component solves it. A change that removes
   lines is progress.
8. **Learn by building.** What can be tried in an afternoon is not designed
   up front. When an attempt is wrong, roll back to the last working state
   rather than patch forward.
9. **Not yet.** Optimization waits for measured slowness, scalability for
   real load, refactoring for the moment the next real change is hard to
   make or hard to explain.
10. **Problems are handled when they appear.** One that breaks an invariant
    is fixed now. Any other is a dial: fix it, cut it, or record it as debt —
    but decide.

### The full statement lives in `prompt.md`; workers get the rules

`prompt.md` carries all of the above with its reasons. Purpose and the dials
are planning decisions: the assistant makes them with the user and writes
them into issues already made — what is in scope, what is not, which crate,
what debt is accepted.

An agent's brief carries the invariants and the habits as rules, without the
dials. **Agents do not turn dials**: they hold none of the conversation the
dials are turned in. A decision the issue does not cover gets the smaller
option.

The two texts come from one source, composed at build time, so they cannot
drift. The fix briefs add one rule of their own: the fix is in the code;
a test or a check changes only if it is itself wrong.

Rejected: the coding rules in the target repository's `CLAUDE.md`, which
agents would read unprompted. `CLAUDE.md` belongs to the user.

### The principles show in the choices, not in the talk

The assistant does not recite these principles to the user or cite them to
justify ordinary decisions. A framework narrated on every call becomes
over-analysis; it is meant to shape judgment quietly.

### The wording is not decided here

The principles are the decision. `prompt.md`'s prose is an implementation of
them and is expected to change by experiment; this entry does not hold it.
A change to a principle, rather than to its wording, is a new entry.

## Consequences

- Epik's throughput is bounded by the user's understanding, by design. Small
  issues, boring code and deviation-only reporting exist to make
  understanding cheap, not to make it unnecessary.
- `prompt.md` is written from this entry. `issue-brief.md` and the two fix
  briefs gain a preamble of invariants and habits from the same source.
- What planning tells implementation, and how a worker reports a departure
  from its plan, is `2026-10-20-what-planning-tells-implementation`.
