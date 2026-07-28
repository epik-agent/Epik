# Theory and Practice

The philosophy behind Epik's approach to software design. It applies to any
project, in any language, at any scale.

## Programming as Theory Building

The framing comes from Peter Naur's *Programming as Theory Building* (1985).
Naur's claim is not merely that understanding matters or that documentation is
important. His claim is stronger: **the theory cannot exist independent of the
activity of programming.** You cannot write it down first and then implement
it. The implementation is how you discover whether your theory is any good.

The theory lives above the code, in the problem domain. A useful way to read
it is through three lenses:

- **Ontology** — what are the things?
- **Processes** — how do they interact?
- **Nomenclature** — what do we call them?

This is a pattern for reading a theory, not a template for writing one. A
theory document is prose; the lenses tell you what to look for in it.

## The Two Objectives

Every project has two objectives in constant tension:

### 1. Develop a theory of the problem

The perfectionist side. Always refactoring, always willing to start over. Its
goal is to *understand* the problem; writing code is a constraint on the kind
of understanding that counts.

### 2. Build a working app

A working app. Always. From the very beginning. This is the **Always Be
Demoing** principle. This side also thinks above the code, but it lives in the
user domain: affordances, user stories, verticals. It presses to get a working
vertical as soon as possible, to have something to show.

### The tension is productive

These objectives are not in opposition. Always Be Demoing is not the enemy of
theory; it is the **experimental method** for theory. Building something that
works is how you discover what your theory gets wrong. The tension is not
theory vs. practice; it is **rigor vs. velocity within a single learning
loop.**

## Reading the Development Process

The development process is not just a sequence of tasks — it is **evidence
about the state of the theory.** Patterns in a project's history are
diagnostic:

- **You've tried to implement the same component three times.** The theory is
  wrong, not the code. The concepts don't carve the problem at its joints.
- **You're suddenly hitting a burst of bugs.** You've outgrown your ontology.
  The abstractions that carried you this far no longer fit.
- **You were moving fast, then hit a hard block.** You were building on
  assumptions that just got falsified.

After a fast productive sprint, ask: "What assumptions made that possible, and
are they actually true?" After hitting a wall: "What concept are we missing?"

Distinguishing "the theory is wrong" from "the implementation is just hard" is
*the* problem of writing software. There is no mechanical solution; it takes
judgment, informed by the project's history.

## The Theory Document

The persistent artifact of this process is not a changelog or a decisions log.
It is a **current best understanding** of the problem, actively revised when
evidence from the development process warrants it.

The theory document should have consequences. An ontology that drives naming
in the code. A process model that maps to the architecture. If the theory
document doesn't constrain future decisions, the tension collapses and you
just have a project with some notes in it.

## A Worked Example: the Electron-to-Plugin Pivot

Epik's own history supplies an example of a theory revision.

The original theory held that Epik was a standalone application: an Electron
chat client and a headless server, connected by a message bus, with the agent
runtime and worker orchestration built in-house. Considerable design effort
went into that architecture — transport protocols, event schemas, process
supervision.

The diagnostic signal was that each new architectural component duplicated
something the surrounding platform already provided. Claude Code already *was*
the chat interface, the agent runtime, and the tool host. The ontology had
placed those things inside Epik, and the friction of rebuilding them was
evidence against the ontology — not evidence that the implementation was hard.

The revision: Epik is not an application; it is a **design partner riding an
existing agent host** — a plugin of commands, skills, and tools. Nearly all of
the architecture was deleted. None of the theory in this document was. That is
what a good theory revision looks like: the durable understanding survives,
and the parts that were really just implementation commitments fall away.
