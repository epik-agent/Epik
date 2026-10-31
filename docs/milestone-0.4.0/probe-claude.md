# The Claude probes (2026-10-31)

Epic A's findings 2 and 3, made with the probe client in
`~/Projects/Epik/codex-probe/` (now with a `--meta` option that sends a
`_meta` object on `session/new`) against `claude-agent-acp` 0.99.0, Claude
Code 2.1.314, in auto mode, on `wpm/epik-test#13`. Each run opened a pull
request into `main`, closed unmerged afterward with its branch deleted.

## Finding 2: the prompt against the skill

Two builds of the same issue, one from A1's prompt alone (the header,
Epik's rules and the issue procedure, no plugin loaded) and one from
`/epik:issue 13 main` with the plugin loaded through
`_meta.claudeCode.options.plugins`, as the app does today.

| | A1's prompt, no plugin | `/epik:issue`, plugin loaded |
|---|---|---|
| Prompt to `end_turn` | 60 s | 61 s |
| Cost (`usage_update`) | $0.17 | $0.19 |
| Tokens | 238k (221k cached reads) | 281k (262k cached reads) |
| Result | PR #18, `Closes #13`, worktree removed | PR #19, same |
| After `end_turn` | nothing | nothing |

**Settled.** The prompt is as good as the skill, a little cheaper, and
what it does is in the record rather than in a plugin the host has to
have. A1 stands as written. (The earlier $1.12 for an ACP build, in the
October record, was a different issue on a different day; this is the
first like-for-like comparison, and one run each.)

Both runs went quiet the moment the prompt returned, as Codex did, so the
300 s quiet period bought nothing here. It stays for Claude because
background subagents are real and a one-issue build doesn't use them.

## Finding 3: Claude Code's sandbox through ACP

The same prompt-only build with this on `session/new`:

```json
{"claudeCode":{"options":{"sandbox":{
  "enabled":true,"failIfUnavailable":true,
  "network":{"allowedDomains":["github.com","api.github.com","objects.githubusercontent.com","index.crates.io","static.crates.io"]},
  "filesystem":{"allowWrite":["<the directory holding the clone>"]}}}}}
```

| Question | Finding |
|---|---|
| Does the adapter take the option? | **Yes.** The session started sandboxed (Seatbelt), `failIfUnavailable` and all. |
| Does `gh` work inside the sandbox? | **No, on macOS.** The first command's `gh issue view 13` failed: `Post "https://api.github.com/graphql": tls: failed to verify certificate: x509: OSStatus -26276`. The sandbox's proxy terminates TLS with its own certificate, and `gh` (Go, using the macOS keychain for roots) doesn't trust it. `ls`, `cat` and `git log` in the same command ran fine. |
| What did Claude do about it? | **Left the sandbox.** Its next message was "The TLS failure is likely the sandbox; retrying outside it", and it reran the command with `dangerouslyDisableSandbox: true`. Auto mode's classifier approved, as `allowUnsandboxedCommands` (default `true`) lets it. Of 11 commands, 6 ran unsandboxed: every `gh` and `git fetch`/`push`, and three that needed no network at all (`cat Cargo.toml …`, `grep …`), because Claude had stopped trusting the sandbox. Edits, tests and the worktree ran sandboxed. |
| Did anything reach Epik? | **No permission request.** The classifier handled each escape. |
| Did the build succeed? | **Yes**, PR #20 in 59 s, at **$0.26** against $0.17 without the sandbox: the failed attempts and the reasoning about them. |
| Would strict mode (`allowUnsandboxedCommands: false`) work? | **Not on macOS as configured:** the build would stop at the first `gh`. It might on Linux, where Go honors `SSL_CERT_FILE` and the proxy's CA could be trusted; untested. `gh` would also need its config and keychain token readable, which it had here. |

**Settled.** With the defaults, Claude's sandbox is advisory: a model
decides to leave it, which is exactly the trust Claude's auto mode already
gives, at half again the cost and with every network command outside the
sandbox anyway. In strict mode it blocks the one thing a build can't do
without. So **the Claude profile runs as today in 0.4.0**, and the record
notes the sandbox as deferred, with the two things that would make it
worth revisiting: `gh` trusting the proxy's certificate (a Linux host may
already), and a strict configuration proven on one build. Codex's `agent`
mode remains the more cautious of the two profiles, and A15's Settings
line can say so plainly.

## Rerunning

```bash
cd ~/Projects/Epik/codex-probe
# Finding 2, the prompt (clone at work/epik-test-claude):
./client/target/debug/codex-probe "$PWD/work/epik-test-claude" runs/prompt.md runs/claude-prompt.jsonl 120 --mode auto -- npx -y @agentclientprotocol/claude-agent-acp@0.99.0
# Finding 2, the skill:
./client/target/debug/codex-probe "$PWD/work/epik-test-claude2" runs/skill-prompt.md runs/claude-skill.jsonl 120 --mode auto --meta '{"claudeCode":{"options":{"plugins":[{"type":"local","path":"/Users/mcneill/Projects/Epik/Epik/plugin"}]}}}' -- npx -y @agentclientprotocol/claude-agent-acp@0.99.0
# Finding 3:
./client/target/debug/codex-probe "$PWD/work/epik-test-claude" runs/prompt.md runs/claude-sandbox.jsonl 120 --mode auto --meta "$(cat runs/sandbox-meta.json)" -- npx -y @agentclientprotocol/claude-agent-acp@0.99.0
```

The logs of the three runs are `runs/claude-prompt.jsonl`,
`runs/claude-skill.jsonl` and `runs/claude-sandbox.jsonl`. The `cwd` must
be absolute: the adapter refuses a relative one. Epic C's Codex findings
are in `runs/codex-*.jsonl` (`codex exec --json`) and
`runs/codex-acp-mcp.jsonl` (through codex-acp), and are written up in
`epic-plan-in-codex.md`.
