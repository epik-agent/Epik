# Epik
_You say it. We make it._

A desktop app: Tauri 2 shell, Leptos frontend, three crates.

- **`crates/epik`** — the domain core, where all logic eventually lives.
  Dependency-free and wasm-clean so that frontend and backend can both depend
  on it and share types — one definition of every message that crosses IPC.
- **`crates/epik-backend`** — the Tauri host: windowing, native capabilities,
  IPC handlers. Depends on `epik`.
- **`crates/epik-frontend`** — the Leptos UI, built by Trunk, styled by
  Tailwind. Deliberately outside the root workspace: it targets
  wasm32-unknown-unknown while the other crates build natively, and a shared
  workspace would entangle the two targets' features and lockfiles.

