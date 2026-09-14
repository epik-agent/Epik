# JSON tracker fixtures

`adder.json` is the issue tracker behind the end-to-end adder test in
`tests/adder.rs`: three issues that together are the Python library in
`src/agent/scripted/fixtures/adder/`. Each issue's body is the relative
paths of the files implementing it, one per line; the files' bytes live
in that one place and the tracker only names them. Issues 2 and 3 depend
on issue 1, so `dependents("1")` is the whole project in an order a plan
can run.
