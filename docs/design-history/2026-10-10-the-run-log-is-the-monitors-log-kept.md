# ADR: The run log is the monitor's log, kept

- **Status:** Proposed
- **Date:** 2026-10-10
- **Source:** Review of the open issues, October 2026. Restates #233, which
  was written before the monitor's log existed.

## Context

A feature build says what it is doing on one stream, a
`plan::stream::Sink<Event<Name>>`. In the app, that sink records each event
into the host's `monitor::Log` as `Change::Happened` (the backend's
`build.rs`), beside the run-level changes the stream does not carry:
`Reserved` before the launch reads the work, `Started` with the plan,
`Finished`, and `Abandoned` with its reason. The log stamps every entry with a
host-wide `seq` and with `at`, the host's clock in unix milliseconds. The
relay and the monitor read it; nothing else does.

The log is in memory. When the app quits, the record of every build it ran
goes with it. A build's tasks return nothing, so the stream is the only
account of what a build did and what it cost —
`the_bill_is_recoverable_from_the_stream_alone` shows that cost and tokens
come back out of a line of JSON per event — and nothing keeps it.

#233 asked for a permanent run log and placed it at the sink: one file per
build written from the stream, events gaining a time, and sinks learning to
combine so the log could sit beside a live view. It was written on
2026-09-21. The monitor's `Log` (#243) landed the next day and has since
answered most of what #233 left open: entries carry a time, and the `Log` is
already the one place every consumer reads from. `build_status`, which #233
named as a further consumer, was retired in #259. The decision #233 cited — a
run log under `~/.epik/logs`, dated 2026-08-08 — was lost before check-in; two
sentences in `2026-08-17-build-from-chat` and
`2026-08-19-a-feature-is-a-build-of-builds` are all that survive of it.

## Decisions

### What is kept is the log's entries

The run log is the monitor's `Log`, written to disk: each `Entry` as
recorded — `seq`, `at` and `change` — as one line of JSON. Not the sink's
events.

- **A build's beginning and end are only in the log.** `Reserved`, `Started`,
  `Finished` and `Abandoned` are run-level changes the stream never carries. A
  record kept at the sink would hold every step of every issue and not say
  what was built, from what plan, or whether it finished. A launch the
  rulesets' policy refuses never reaches the sink at all, and it is exactly
  the run a person goes looking for.
- **One representation.** The monitor holds that what a watcher gets on attach
  and what it builds by listening cannot disagree, because there is one
  representation and one fold. A file of entries is that representation on
  disk, folded by the same `Progress::absorb`. A file of sink events would be
  a second format with a second reader.
- **The time is already stamped.** `Entry::at` is what #233 left open —
  whether the writer stamps the line or `Event` gains the field. `Event` stays
  without a time.

### One file per run, named for what it was

Entries are host-wide; a file is one run's. The recorder writes each entry to
its run's file, found by `Change::run`.

`RunId` and `seq` both count from their start each time the app starts, so
neither names a file. A run's file is created at its `Reserved` and named from
it: `Reserved`'s `at` as a sortable UTC time, then the repository and the
feature — `20261010T193412Z-epik-agent-Epik-284.jsonl`. A feature's runs sort
by when they ran and say which feature they were of. The `seq` inside a file
is the host's: increasing, not dense.

The files live under the app's data directory, in `runs/` beside
`worktrees/`, not under `~/.epik/logs`. Epik keeps its state where Tauri puts
it; the lost decision's path predates the desktop app.

### The recorder is a reader of the log

The log calls no one: delivery is a reader's, so that every reader hears every
entry in `seq` order however many threads are recording. The recorder is one
more reader beside the relay. It follows the log with `wait_after` and a
cursor of its own, and appends each entry to its run's file, flushed line by
line.

So the record trails the log by however long the recorder takes to wake. A
process that dies leaves every entry the recorder had reached, each line
whole; an entry recorded in the last instant before the death can be missing.
Accepted: the alternative is file I/O under the log's mutex, on every record,
on every thread that emits.

### Recording is the app's default; keeping nothing has a name

The app attaches the recorder where it makes its log, at startup, so every
build the app runs is recorded and no build has to ask. A host that keeps no
record — a test, a dry run — makes a `Log` and attaches no recorder, and that
choice is visible where the log is made.

At the stream, `Sink::null()` hears everything and keeps nothing. It is what
`Sink::new(drop)` does today in `tests/adder.rs`, named, so that a reader or a
grep finds a build that keeps no account. Sinks do not learn to combine: the
log is where the stream fans out.

### A record that cannot be written stops no build, and says so once

A run whose file cannot be created or appended to goes on building; the log in
memory, and so the monitor, are untouched. The recorder reports the first
failure for a run once, as a change in the log —
`Change::Unrecorded { run, reason }` — so it reaches the monitor the way
everything else does and shows on that run's tab. Then it stops writing that
run rather than failing again at every entry. `Unrecorded` is never itself
written.

### What goes in is what the stream carries

Nothing typed `Secret` is in a `Change`, and that stays true. An agent's
narration and stderr are recorded as they came. A run file can therefore
hold source the agent read and printed; it lives in the user's data directory,
beside the user's clones.

### Not decided here

- **Restoring the monitor from the files at startup.** It is the natural next
  use, and it is not free. `seq` and `RunId` restart with each launch, so
  restored runs need their entries re-sequenced into the new log and their ids
  re-issued; and a file with neither `Finished` nor `Abandoned` is a run that
  was interrupted, which the monitor has no stage for.
- **Retention.** Files accumulate. A build makes a handful of entries per issue;
  nothing is pruned until that is a problem someone has.

## Rejected

- **Writing at the sink (#233 as written).** It misses the run-level changes,
  needs sinks to combine, and puts a second format on disk.
- **Writing inside `Log::record`.** Exact — nothing recorded is ever missing
  from the file — at the price of disk I/O under the log's mutex, serializing
  every emitting thread behind a flush. And the log's own design is that it
  calls no one.
- **One file for the host.** Simpler to write; a run's record is then a filter
  over every run the app ever made, interleaved.

## Consequences

- `monitor`: `Change::Unrecorded`, folded onto its run's `Watch`; a recorder —
  a reader of a `Log` with a directory and a cursor; and a reader of a run's
  file back into entries. The recorder and the file reader are in the
  `native` half, beside `Log`.
- `plan::stream`: `Sink::null()`.
- The backend attaches the recorder to the app's log at startup, under
  `app_data_dir()/runs`.
- A run's bill — cost and tokens, per issue and in total — is a fold over its
  file.
- Tests name their own directory; none writes under the app's data directory.
