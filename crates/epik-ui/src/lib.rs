//! The Epik window's frontend.
//!
//! Nothing lives here that is not narrowly concerned with user interface.
//! Domain types, and the truth they describe, come from `epik`; this crate
//! decodes them and renders them.
//!
//! Only the modules that touch the DOM or the host are wasm-only. The rest —
//! the fold that turns an event stream into what the user sees, the markdown
//! it renders, the theme it renders in — compiles and tests on the host target,
//! which is where their tests run and where no browser is ever needed.

pub mod ipc;
pub mod markdown;
pub mod pane;
pub mod theme;

#[cfg(target_arch = "wasm32")]
pub mod app;
#[cfg(target_arch = "wasm32")]
pub mod bridge;
