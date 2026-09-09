# Claude Code stream-json fixtures

Captured from the real `claude` CLI (2.1.233 (Claude Code)) on
2026-08-15, using exactly the flags [`ClaudeCode`] builds into its
Task, against a scratch directory in a tempdir.

- `session.jsonl` — one complete successful session: the prompt was
  "create a file named hello.txt containing exactly the word hello and
  nothing else". Ten lines: session hooks, `system`/`init`, thinking
  estimates, an assistant thinking block, an assistant `tool_use`
  (Write), the tool result, an assistant text block, and the
  `type: "result"` line with `subtype: "success"`.
- `result_error.json` — the terminal result line of a session run with
  `--max-turns 1` on a task needing more: `is_error: true`,
  `subtype: "error_max_turns"`, no `result` text, the reason under
  `errors`.

Recapture:

```sh
cd "$(mktemp -d)"
echo "create a file named hello.txt containing exactly the word hello and nothing else" \
  | claude -p --output-format stream-json --verbose \
      --settings '{}' --strict-mcp-config --dangerously-skip-permissions \
  > session.jsonl

echo "run ls, then create files a.txt, b.txt, and c.txt with separate writes" \
  | claude -p --output-format stream-json --verbose \
      --settings '{}' --strict-mcp-config --dangerously-skip-permissions \
      --max-turns 1 | tail -1 > result_error.json
```

The interpreter is deliberately unknown-tolerant, so a newer CLI's
extra fields or event types must not break the tests; recapture when
the *modelled* fields move.

[`ClaudeCode`]: ../../claude_code.rs
