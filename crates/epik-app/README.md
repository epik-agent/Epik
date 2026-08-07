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

## macOS: signing and the keychain

The keychain recognises an app by its code signature. An unsigned build gets a
fresh ad-hoc signature every rebuild, so every rebuild is a stranger to the
keychain: each stored `Epik` item costs an access dialog again. Signed with a
stable identity, the app is the same app across rebuilds — a dialog happens at
most once per stored item, because answering it Always Allow now names an
identity that survives the next build — and never for items the app created
itself, because the creator is on an item's access list from the start.

The identity is supplied by the environment, never hardcoded in
`tauri.conf.json`: `cargo tauri build` reads `APPLE_SIGNING_IDENTITY` and signs
the bundle with whatever it names. A development certificate on a dev machine
and Developer ID in the release job are then the same build path with a
different value exported.

### One-time setup on a dev machine

Create a self-signed code-signing certificate in Keychain Access — Certificate
Assistant → Create a Certificate, name it `Epik Development`, set Certificate
Type to Code Signing — then export its name wherever your shell learns its
environment:

```sh
export APPLE_SIGNING_IDENTITY="Epik Development"
```

`cargo tauri build` now produces a bundle the keychain will keep recognising.
The dev-server path (`cargo tauri dev`, plain `cargo run`) still links an
ad-hoc-signed binary whose identity changes per rebuild, so keyring dialogs
there are expected — iterate with `EPIK_API_KEY`/`EPIK_GITHUB_TOKEN` exported,
or use the signed bundle when the keychain is the thing being exercised.

If pre-signing builds ever touched your keychain, their `Epik` items carry
access lists naming identities that no longer exist, and the dialogs will not
stop until those items go. Delete them once —

```sh
while security delete-generic-password -s Epik >/dev/null 2>&1; do :; done
```

— then re-paste keys through the app's own cards, which files them with the
signed app on the access list.

### Release

The release job exports the same variable naming a `Developer ID Application:
…` certificate, plus the notarization credentials Tauri's bundler reads:
`APPLE_ID`, `APPLE_PASSWORD` (an app-specific password), and `APPLE_TEAM_ID` —
or an App Store Connect API key via `APPLE_API_KEY`, `APPLE_API_ISSUER`, and
`APPLE_API_KEY_PATH`. Same `cargo tauri build`, nothing forked.

### Elsewhere

This section is macOS-only on purpose. Linux's Secret Service and Windows's
Credential Manager gate secrets per user, not per app: no access list names a
binary, so signing never enters into secret access, and the only prompt Linux
can raise — unlocking a locked collection — asks about the user's keyring,
never about which app is asking. The launch discipline the library keeps —
each secret read once — still holds everywhere; macOS is just the platform
where each read can cost a dialog.

## When nothing streams

If the window opens and the status bar names a model, but a question never gets
an answer and the input never comes back, the event channel is being denied
rather than the model failing — `capabilities/default.json` is what grants it,
and its absence produces exactly that symptom with no error anywhere.
