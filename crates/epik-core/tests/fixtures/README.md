# Shared fixtures

`adder/` is the project the tests build end to end: a Python library
with one function, `add`, the least a uv project needs to build and
test it, one pytest test, and `issues.json`, the issue tracker whose
three issues together are that library. Each issue's body is the
relative paths of the files implementing it, one per line, so the
files' bytes live here once and the tracker only names them. Issue 0
contains the three as sub-issues, and issues 2 and 3 depend on issue 1,
so `work("0")` is the whole project in an order a plan can run.

Who reads it: the scripted engine in `../support/scripted.rs` includes
the source files as `adder()`, the JSON tracker in
`../support/json_tracker.rs` reads `issues.json`, and `adder.rs` runs
the engine over both.

To check the project itself stands up:

```sh
cd adder && uv run pytest
```
