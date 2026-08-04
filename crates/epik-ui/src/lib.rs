//! The Epik window's frontend.
//!
//! Nothing lives here that is not narrowly concerned with user interface.
//! Domain types, and the truth they describe, come from `epik`; this crate
//! decodes them and renders them.
//!
//! Only the modules that touch the DOM are wasm-only. The rest — the pure
//! functions that turn an event stream into what the user sees — compile and
//! test on the host target, which is where their tests run.

pub mod ipc;

#[cfg(target_arch = "wasm32")]
pub mod app;
