# Epik
_You say it. We make it._

A desktop app: Tauri 2 shell, Leptos frontend, two crates.

- **`crates/epik-core`** — the library, where the logic lives.
  Dependency-free and wasm-clean so that frontend and backend can both depend
  on it and share types — one definition of every message that crosses IPC.
- **`crates/tauri/backend`** — the Tauri host: windowing, native capabilities,
  IPC handlers. Depends on `epik-core`.
- **`crates/tauri/frontend`** — the Leptos UI, built by Trunk, styled by
  Tailwind. Deliberately outside the root workspace: it targets
  wasm32-unknown-unknown while the other crates build natively, and a shared
  workspace would entangle the two targets' features and lockfiles.

The tests are written to one nomenclature, described in
[`docs/testing.md`](docs/testing.md); `docs/design-history` holds the
decisions that got us here.

