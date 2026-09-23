# How Epik's tests are written

`epik-core` holds four hundred-odd tests. They already read one way in
the large; what follows is that way written down, so the next test
joins them rather than starting a second dialect.

This is a description, not a decision: `docs/design-history` records
choices and is never edited, while this file is amended whenever the
tests move.

## A test's name states one claim

Article-led sentences, read as prose:

```rust
fn a_stored_secret_comes_back() { … }
fn the_first_failure_in_node_order_is_the_plans() { … }
```

No `test_` prefix. A name that needs an "and" joining two independent
claims is two tests. An "and" within one claim is fine: a name about a
value crossing the wire and coming back itself is one round trip, while
a name listing what three separate assertions check is three tests.

A name that says only a mechanism is not yet a claim. `stdout_is_delivered`
names a pipe; `what_a_process_prints_arrives_on_stdout` names what the
test would catch if it broke.

## One word, one meaning

A word used in the tests names one thing across the whole crate. Where
two things want the same word, both get longer ones:

- `real` is the block of tests that run the real `git` binary, always
  compiled where git is; `live` is kept for tests that reach an outside
  service and are asked for by hand.
- `sh` builds a shell process; `collected` runs one and reads it back.
- `change` builds a `Change`; `recorded` reads events back out.

A helper meaning the same thing in two places is shared rather than
copied. `dag::shape` — the edges of a `Dag`, as a list — is one generic
function in `dag`'s testing surface, used by `tracker` and by the
integration tests alike.

## A fixture is a past participle naming the state it leaves

`identified`, `committed`, `aimed`, `guarded`, `published`, `unpushed`:
each says what the world looks like once it has run, not what it did.
Two fixtures leaving the same state share the name and are told apart
by their arguments.

## A double says what kind of double it is

Two words, each naming its subject:

- **`Fake*`** impersonates a real service — `FakeGitHub` answers the
  requests GitHub would.
- **`Scripted*`** follows a script the test writes — `ScriptedModel` is
  a loopback model provider, `ScriptedAgent` an `Agent` that shells out.

## Scratch labels name the test that left them

`Scratch::new("<module>-<case>")`, so a scratch directory surviving a
crash says which test to look at: `"worktrees-late"`, `"commands-merge"`.
The module part is the file's name, the case part enough of the test to
tell it from its neighbours.

## Assertions

`assert_eq!` by default: it prints both sides. `assert!` is for what
has no other side — a predicate, a `matches!`, an `is_err` — and every
one carries a message. Where a value explains the failure, the message
is that value (`"{error:#}"`); where it does not, it is prose saying
what was expected.

## `unwrap()` in tests

`unwrap()` on anything a test needs to have worked. `expect("…")` only
where the message is itself the assertion, stating what the test
believes: `expect("a checked-out branch")`, `expect("one result")`.

## Test modules

One `mod tests` per file. A sibling module gated differently is named
for its gate and not suffixed: `unix`, `real`, `live`, `log`. Helpers
inside a test module are private unless the `testing` feature exports
them for dependent crates.

## Live tests

A test that reaches a service outside this machine is gated at runtime
by an `EPIK_LIVE_<SUBJECT>` variable, and skips with a word on stderr
when it is unset:

```rust
if std::env::var("EPIK_LIVE_CLAUDE").as_deref() != Ok("1") {
    eprintln!("EPIK_LIVE_CLAUDE is unset; skipping");
    return;
}
```

Runtime, so that `cargo test` compiles them either way and a typo in a
gate fails loudly. `cfg` gates stay for platform and feature, which ask
a different question — whether the code exists at all.

Today that is `EPIK_LIVE_CLAUDE`, for the one test that runs the real
`claude` CLI, and `EPIK_LIVE_BASE_URL` with `EPIK_LIVE_MODEL`, which
point the assistant's live tier at a provider.

## Fixture files live in one place

A captured fixture read from both a unit test and an integration test
lives beside the code that parses it, and the integration test reaches
it through the same `testing` surface rather than through a relative
path into `src/`.
