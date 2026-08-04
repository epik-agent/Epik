//! Tauri's codegen embeds the frontend bundle at compile time, so
//! `frontendDist` has to exist before `cargo build` runs — but Trunk, which
//! produces it, is not part of the Cargo build graph. Rather than make every
//! `cargo test --workspace` depend on a wasm toolchain, stand a placeholder
//! in the empty case. A real Trunk build overwrites it.

use std::fs;
use std::path::Path;

const PLACEHOLDER: &str = "<!doctype html>\n<title>Epik</title>\n\
    <p>The frontend has not been built. Run <code>trunk build</code> in \
    <code>crates/epik-ui</code>.</p>\n";

fn main() {
    let dist = Path::new("../epik-ui/dist");
    let index = dist.join("index.html");
    if !index.exists() {
        let _ = fs::create_dir_all(dist);
        let _ = fs::write(&index, PLACEHOLDER);
    }
    tauri_build::build();
}
