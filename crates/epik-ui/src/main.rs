//! Mounts the window's frontend.

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(epik_ui::app::App);
}

/// On the host target this crate exists only to be tested: its DOM-facing
/// modules are wasm-only, so there is nothing to mount.
#[cfg(not(target_arch = "wasm32"))]
const fn main() {}
