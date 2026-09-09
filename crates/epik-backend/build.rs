fn main() {
    // The monitor embeds the frontend bundle from the directory Trunk
    // emits. A build that has not run Trunk — clippy in CI, a fresh
    // checkout — still has to compile, so the directory is made to
    // exist, empty, before `include_dir!` reads it; and a bundle that
    // changes is a reason to build the crate again.
    let dist = concat!(env!("CARGO_MANIFEST_DIR"), "/../epik-frontend/dist");
    std::fs::create_dir_all(dist).expect("the bundle directory can be made");
    println!("cargo:rerun-if-changed={dist}");
    tauri_build::build()
}
