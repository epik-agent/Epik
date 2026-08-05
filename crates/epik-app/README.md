# Running the Epik window

The host embeds the frontend bundle at compile time, and the frontend is not
part of the Cargo build graph. So the frontend is built first, always:

```sh
cd crates/epik-ui && trunk build
```

Then, for a window you can iterate in — `trunk serve` reloads the frontend, and
Tauri points at it rather than at the bundle:

```sh
cargo tauri dev            # from crates/epik-app
```

Or, for the window as it ships:

```sh
cargo run --release -p epik-app --features custom-protocol
```

That feature is not optional. Without it Tauri looks for the dev server, finds
nothing listening, and shows `Could not connect to localhost` instead of the
window. `cargo tauri build` passes it for you.

## Pointing it at a model

Configuration lives at `~/.epik/config.toml`, or wherever `EPIK_HOME` says. It
is edited by hand — there is no settings UI, by decision — and the status bar
names the model that took, so you can see whether an edit landed.

The free local path, which needs no key and costs nothing:

```sh
ollama serve &
ollama pull smollm2:135m
```

```toml
active = "ollama"

[providers.ollama]
base_url = "http://localhost:11434/v1"
model = "smollm2:135m"
```

Any endpoint speaking the OpenAI chat-completions format works the same way —
that is where model-agnosticism comes from here. For one that wants a key, paste
it into the card the window shows: it goes to the OS keyring under the service
`Epik`, and never into this file. `EPIK_API_KEY` overrides the keyring for one
shell session, which is also the way in on a machine that has no keyring at all.

## When nothing streams

If the window opens and the status bar names a model, but a question never gets
an answer and the input never comes back, the event channel is being denied
rather than the model failing — `capabilities/default.json` is what grants it,
and its absence produces exactly that symptom with no error anywhere.
