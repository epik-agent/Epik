# The Codex probe (2026-10-31)

Epic A's finding 1. **Codex built `wpm/epik-test#13` unattended in its
default `agent` mode**, from A1's prompt alone, and opened
[wpm/epik-test#17](https://github.com/wpm/epik-test/pull/17) into `main`
with `Closes #13`. It was closed unmerged afterward, with its branch deleted.

A first attempt stopped at its first turn: the OpenAI API key's account had no credits. The
second ran after `codex login` with ChatGPT.

## The run

- **71 s** from prompt to `end_turn`, then 300 s of silence until the
  quiet period ended. Model `gpt-6.1-sol` at its default reasoning effort
  `low`.
- It followed the procedure: read the issue, made a worktree
  (`/private/tmp/epik-issue-13`, branch `issue/13-exponentiation-rebuild`),
  wrote tests and saw them fail, implemented, ran `cargo test`, `clippy`
  and `fmt`, committed, pushed, opened the PR, ran
  `gh pr checks --watch --fail-fast` (the repo has no checks), and removed
  the worktree. 32 lines added, 1 deleted, in `src/main.rs` and
  `tests/cli.rs`.
- **Codex's automatic reviewer approved every action that needed it**:
  each `gh` call, `git fetch`, the worktree outside the clone, the commit
  and push, and `gh pr create`. Each review took 1.5 to 3.5 s. **No
  permission request reached the client**, so rejecting every request, as
  Epik does today, cost nothing.
- **Tokens, not cost:** the `session/prompt` response carries standard
  ACP `usage` (222,223 total, 200,576 of them cached reads, 1,925 output),
  and `_meta.quota` repeats them per model. `usage_update` holds only the
  context window (23k used of 475k). No dollar figure anywhere.

## What it settled anyway

| Question | Finding | Source |
|---|---|---|
| Does `agent-client-protocol` 3.6 talk to codex-acp 2.5.0? | **Yes.** `initialize`, `session/new`, `session/set_config_option` and `session/prompt` worked with no client capabilities, as Epik sends them today. The adapter started in under a second once cached, and about 6 s on its first download. | Probe |
| Is the unattended setting a mode or a config option? | **Both.** The adapter offers ACP session modes `read-only`, `workspace-write`, `agent` (default) and `agent-full-access`, and the same four as a config option `mode` of category `mode`. `INITIAL_AGENT_MODE` sets the starting one. So Epik's `SetSessionModeRequest` works unchanged: no change to `agent.rs:198-215`. | Probe, adapter source |
| Can `CODEX_CONFIG` give a sandbox with network access that never asks? | **No, not through 2.5.0.** Every turn sends its mode's approval and sandbox policy explicitly (`sendPrompt`, `index.js:42069`), which overrides the session config. Every sandboxed mode has `networkAccess: false`. Only `agent-full-access` has approval `never`. | Adapter source |
| What does the default `agent` mode do with a risky action? | Workspace-write sandbox with no network, approval `on-request`, and approvals answered by Codex's **automatic reviewer** (`approvalsReviewer: auto_review`), not the user. This is the nearest thing to Claude's auto mode. **Unverified:** whether the reviewer approves `git push` and `gh pr create`, and whether anything reaches the client as a permission request. | Adapter source |
| The `model` option's values | `gpt-6.1-sol` (default), `gpt-6-astra`, `gpt-6.1-terra`, `gpt-6-sol`, `gpt-6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`. Other options: `reasoning_effort` (category `thought_level`, default `low`; `low` to `ultra`), `fast-mode` (`off` by default), `collaboration_mode` (`default` or `plan`). | Probe |
| Which Codex does the adapter run? | **Its own.** codex-acp 2.5.0 depends on `@openai/codex` ^0.165.0 and runs that copy (354 MB for macOS ARM64) unless `CODEX_PATH` says otherwise. This Mac's Homebrew Codex is 0.145.0, older than the adapter expects. It shares `~/.codex`, so the sign-in is the same. | Package, npx cache |
| How is sign-in reported? | `initialize` lists auth methods `api-key` and `chat-gpt`, and `_meta.authStatus`, which is empty here. `NO_BROWSER=1` hides the ChatGPT method. | Probe, README |
| What does a quota or billing failure look like? | `session/prompt` fails with JSON-RPC "Internal error", data `{"message": "Quota exceeded. Check your plan and billing details.", "codexErrorInfo": "usageLimitExceeded"}`, after an `agent_message_chunk` with the same text. Today Epik would record "agent_failed: Internal error: …". | Probe |

## Settled by the run

| Question | Finding |
|---|---|
| Does the `agent` mode's reviewer let `git push` and `gh pr create` through? | **Yes**, every time, with no permission request reaching the client. `agent-full-access` isn't needed. |
| Does the prompt return before background terminals finish? | **No background work was left.** The last command finished before `end_turn`, and nothing arrived in the 300 s after it. For Codex the quiet period only adds five minutes to every issue. |
| Does `rawOutput` hold command output with no terminal capability? | **No.** `execute` calls carry `rawInput.command` and `cwd`, content of type `terminal`, and the output in `_meta.terminal_output_delta` and `_meta.terminal_exit`. A9 reads commands from `rawInput.command`; output, if the stream wants it, comes from that `_meta`. |
| What does `usage_update` hold? | The context window only. Tokens are in the prompt response's `usage`. |
| Does A1's prompt alone get Codex to a pull request with `Closes #13`? | **Yes**, at the first try. |
| How do edits look? | `edit` calls with `rawInput` null and content of type `diff`. The automatic reviewer shows as tool calls of kind `think` titled "Guardian Review". |

## Still open

- One run says only that nothing broke. A larger issue, or one with
  failing checks, would test the "fix until checks pass" loop.
- Whether the reviewer ever refuses something Epik's procedure needs, and
  what the client sees then.
- Whether Epik should shorten the quiet period for an agent whose prompt
  returns only when its work is done.

## What it changes in the plan

- **A6**: drop the `setup::codex` locator and `CODEX_PATH`. The adapter
  brings its own Codex, so Epik passes no path. The unattended setting is
  a session mode. The decision narrows to `agent` against
  `agent-full-access` (see below).
- **A7**: the `codex` item checks that the adapter's Codex runs, not a
  Codex on `PATH`. `codex-auth` reads the adapter's auth status or
  `~/.codex/auth.json`, not the Homebrew `codex login status`.
- **A8 (private Codex install)**: **probably drop.** The npm adapter
  installs Codex with itself, as `claude-agent-acp` brings its own SDK.
  Keep it only if the probe shows the bundled copy failing.
- **A6, errors**: a failure carrying `codexErrorInfo` names the reason.
  "Codex's account is out of credits" should reach the run page, not
  "Internal error".
- **Decision 2**: recommend the `agent` mode. It is sandboxed, Codex's own
  reviewer approves what leaves the sandbox, and it built #13 unattended.
  `agent-full-access` isn't needed.
- **A6, cost**: Codex reports tokens in the prompt response's `usage`.
  Still no money, so "not reported" stands; the record can keep the
  tokens.
- **A9**: Codex's commands are `rawInput.command`, its output is in
  `_meta.terminal_output_delta`, and its reviewer shows as `think` calls
  titled "Guardian Review", which the activity line should skip or label.

## Rerunning

Everything is in `~/Projects/Epik/codex-probe/`:

- `client/`: the probe, a Rust ACP client on `agent-client-protocol`
  3.6.0 that behaves like `epik-core`'s `ClaudeAcp`: no capabilities,
  every permission rejected, one prompt, a quiet period. It logs every
  line, with time, to a JSONL file.
- `runs/prompt.md`: A1's prompt for `wpm/epik-test#13`: a header, then
  `rules.md`, then `procedure.md` from `main`.
- `tools/summarize.py`: tool calls, permissions, usage and gaps from a log.
- `work/epik-test`: a fresh clone. Its `main` has no exponentiation; the
  October Claude PRs (#14 to #16) were closed unmerged.

```bash
cd ~/Projects/Epik/codex-probe
./client/target/debug/codex-probe work/epik-test runs/prompt.md runs/agent-mode.jsonl 300 -- npx -y @agentclientprotocol/codex-acp@2.5.0
python3 -I tools/summarize.py runs/agent-mode.jsonl
```

`runs/agent-mode.jsonl` is the successful run's full log. For full
access, add `--mode agent-full-access` before `--`. Each run opens a pull
request on `wpm/epik-test`, to be closed unmerged afterward, as #14 to #16
were.
