# Scripted engine fixtures

`adder/` is the project the scripted engine's test writes: a Python
library with one function, `add`, the least a uv project needs to
build and test it, and one pytest test. The test in `../../scripted.rs`
includes each file by path and asserts the engine wrote it whole.

To check the project itself stands up:

```sh
cd adder && uv run pytest
```
