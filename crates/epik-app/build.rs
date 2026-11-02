#[cfg(feature = "desktop")]
use std::path::Path;

fn main() {
    // Without the desktop there are no windows to embed, and no Tauri.
    #[cfg(feature = "desktop")]
    if windows_built() {
        tauri_build::build()
    }
}

/// Whether the windows trunk builds, which the app embeds as it compiles,
/// are there. Without them `tauri::generate_context!` panics with a
/// message that doesn't say how to build them, so this says it instead,
/// and fails the build.
#[cfg(feature = "desktop")]
fn windows_built() -> bool {
    let config =
        std::fs::read_to_string("tauri.conf.json").expect("epik-app has a tauri.conf.json");
    let config: serde_json::Value = serde_json::from_str(&config).expect("tauri.conf.json is JSON");
    let dist = config["build"]["frontendDist"]
        .as_str()
        .expect("tauri.conf.json names its frontendDist");
    let index = Path::new(dist).join("index.html");
    // Checked again once they are built, or if they go.
    println!("cargo::rerun-if-changed={}", index.display());
    if index.exists() {
        return true;
    }
    println!(
        "cargo::error=The app embeds Epik's windows, which aren't built: {} is missing. \
         Build them first with `(cd crates/epik-ui && trunk build)`, \
         and again after changing crates/epik-ui. CONTRIBUTING.md says more.",
        index.display()
    );
    false
}
