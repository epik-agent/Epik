# Captured chat-completions streams

Two providers speaking the same protocol differently, which is the point: the
decoder is tested against what providers actually send, not against what the
spec says they should.

| File | Provenance |
| --- | --- |
| `ollama.sse` | Captured verbatim from Ollama 0.13 serving `smollm2:135m`, `max_tokens: 5` |
| `openai.sse` | OpenAI's documented `chat.completion.chunk` shape, written out by hand — reaching the real endpoint needs a key, and no test of Epik's may need one |
| `ollama-tools.sse` | Ollama's OpenAI-compatible tool-call shape (a `tools`-capable model such as `qwen2.5:0.5b`), written out by hand to the shape Ollama emits |
| `openai-tools.sse` | OpenAI's documented tool-call streaming shape, written out by hand, for the same reason as `openai.sse` |

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

And for the tool-call streams:

- OpenAI opens each call with a delta carrying `index`, `id`, `type`, the
  function name, and an **empty** `arguments` string, then streams the
  argument JSON in fragments — subsequent deltas carry only `index` and an
  `arguments` piece, and the pieces concatenate into one JSON string.
  `openai-tools.sse` asks for two calls in parallel, so `index` is doing real
  work.
- Ollama sends each call **whole** in a single delta: id, name, and the
  complete `arguments` string at once, with `"content": ""` beside it. It
  also orders `id` before `index`, where OpenAI does the reverse.
- Both end the turn with `"finish_reason": "tool_calls"` — how a turn that
  ended asking for tools is told apart from one that ended in text — before
  the usual usage-only chunk.

To recapture the Ollama ones (the tool-call capture needs a model that can
call tools, which `smollm2:135m` is not):

```sh
curl -sN http://localhost:11434/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"smollm2:135m","messages":[{"role":"system","content":"You are Epik."},{"role":"user","content":"Hi"}],"stream":true,"stream_options":{"include_usage":true},"max_tokens":5}'

curl -sN http://localhost:11434/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"qwen2.5:0.5b","messages":[{"role":"user","content":"What is the weather in Paris?"}],"tools":[{"type":"function","function":{"name":"get_weather","description":"Today'\''s weather in a city.","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}],"stream":true,"stream_options":{"include_usage":true}}'
```
