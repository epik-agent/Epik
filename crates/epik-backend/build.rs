fn main() {
    // The monitor embeds the frontend bundle from the directory Trunk
    // emits. A build that has not run Trunk — clippy in CI, a fresh
    // checkout — still has to compile, so the directory is made to
    // exist, empty, before `include_dir!` reads it; and a bundle that
    // changes is a reason to build the crate again.
    let dist = concat!(env!("CARGO_MANIFEST_DIR"), "/../epik-frontend/dist");
    std::fs::create_dir_all(dist).expect("the bundle directory can be made");
    println!("cargo:rerun-if-changed={dist}");
    // `trunk serve` writes a bundle meant for its own server — its
    // autoreload client dials a placeholder address — and embedding it
    // is a mistake worth a word at build time, since the server refuses
    // to serve it.
    let index = std::fs::read_to_string(format!("{dist}/index.html")).unwrap_or_default();
    if index.contains("__trunk_address__") {
        println!(
            "cargo:warning=crates/epik-frontend/dist is a trunk serve bundle; \
             run trunk build before embedding the monitor page"
        );
    }
    tauri_build::build()
}
