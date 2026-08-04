# Captured chat-completions streams

Two providers speaking the same protocol differently, which is the point: the
decoder is tested against what providers actually send, not against what the
spec says they should.

| File | Provenance |
| --- | --- |
| `ollama.sse` | Captured verbatim from Ollama 0.13 serving `smollm2:135m`, `max_tokens: 5` |
| `openai.sse` | OpenAI's documented `chat.completion.chunk` shape, written out by hand — reaching the real endpoint needs a key, and no test of Epik's may need one |

Where they differ, and why each difference is worth a fixture:

- Ollama tags **every** delta with `"role": "assistant"`; OpenAI sends the role
  once, in an opening delta whose content is empty.
- OpenAI's final content chunk carries `"delta": {}` — no `content` key at
  all — where Ollama sends `"content": ""`.
- Both end with a usage-only chunk carrying `"choices": []`, so indexing
  `choices[0]` would panic on a real stream from either.
- OpenAI nests `prompt_tokens_details` and `completion_tokens_details` inside
  `usage`, and decorates chunks with `logprobs`, `service_tier`, and
  `refusal`. None of it is modelled; none of it may break decoding.

To recapture the Ollama one:

```sh
curl -sN http://localhost:11434/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"smollm2:135m","messages":[{"role":"system","content":"You are Epik."},{"role":"user","content":"Hi"}],"stream":true,"stream_options":{"include_usage":true},"max_tokens":5}'
```
