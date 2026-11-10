# Contributing to Epik

## Layout

- [`plugin/`](plugin/README.md) — the **epik** plugin: skills and
  configuration. Its tools come from the Epik app.
- [`crates/`](crates) — the **Epik app** (`epik-app`), Epik's only program: it
  runs builds and, as `epik-app mcp`, serves Epik's MCP tools, running its
  GitHub operations through the `gh` CLI. `epik-core` is the library it is
  built on, and `epik-view` holds what its windows show.
  [`docs/app-protocol.md`](docs/app-protocol.md) is how the MCP server talks
  to the running app.
- [`website/`](website/README.md) — the static Epik website, deployed to
  GitHub Pages on every push to `main` that passes CI.

## Building from source

For contributors. You need [Rust](https://www.rust-lang.org/), stable,
installed with [rustup](https://rustup.rs/), and Node 26 or later for the
ACP adapter the app runs. On Linux, the app also needs the system packages
Tauri needs; see Tauri's
[Linux prerequisites](https://v2.tauri.app/start/prerequisites/#linux).

The app's windows are Rust compiled to WebAssembly by
[trunk](https://trunkrs.dev), and the app embeds them when it compiles, so
build them first:

```
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
(cd crates/epik-ui && trunk build --release)
cargo install --path crates/epik-app
```

Build them again with `trunk build` after changing `crates/epik-ui`, before
`cargo build`, or let `cargo tauri dev`, run in `crates/epik-app`, build
them.

Run it once (`epik-app --version` will do): each time it runs it links
itself at `~/.epik/bin/epik-app`, which is where the plugin finds Epik's
tools. Where there is no tray, such as a server without a desktop, the app
runs without one and builds the same. To try the plugin from a clone, see
[`plugin/README.md`](plugin/README.md).

### The macOS installer

The release workflow builds the universal `.dmg` with `cargo xtask dmg`,
which a Mac with the Tauri CLI and both Apple targets can run too:

```
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo install tauri-cli --version 2.12.4 --locked
cargo xtask dmg
```

The build signs when `APPLE_SIGNING_IDENTITY` is set and notarizes when
`APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` are, and the check that
follows demands each of those the environment promised. Notarization
credentials are repository secrets, so a local build is signed at most;
`cargo xtask check-dmg [path]` checks a `.dmg` already built, such as
one downloaded from a release.

With `TAURI_SIGNING_PRIVATE_KEY` set, the build also makes the update
files, `Epik.app.tar.gz` and its `.sig`. `cargo xtask latest-json
<version> <tag> <dir>` writes the manifest the updater reads for the
update files in `<dir>`, as the release workflow does for each final
release.

## Making a release

1. Bump the workspace version in the root `Cargo.toml` in a pull request,
   and merge it.
2. On the Actions page, choose Release, then Run workflow. Leave the
   suffix empty for the final release, or give one, such as `rc1`, for a
   release candidate.
3. The workflow tags the commit `v<version>` or `v<version>-<suffix>`,
   builds the installers, and drafts the release. It fails if the tag
   already exists. If the draft already exists, it uploads onto it.
4. If a job fails, fix the cause and choose Re-run failed jobs on that
   run, which keeps the tag. Running the workflow again would refuse the
   tag that already exists. To abandon the run instead, delete the tag
   with `git push --delete origin v<version>` and the draft, then run the
   workflow again.
5. Publish the draft on the Releases page.

A release is never created on the Releases page, which publishes it before
anything is built.
