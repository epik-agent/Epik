//! Epik's library: everything Epik can do.
//!
//! Anything the app can do must be achievable through calls into this crate
//! alone. That invariant is what keeps every future thin host possible — the
//! remote daemon, the worker, an MCP server facing another chat client — and
//! it is why the Tauri crate next door holds a window and nothing else.
//!
//! The crate stays synchronous: blocking I/O and channels, no tokio. Async is
//! the host's problem.
//!
//! It also stays compilable for `wasm32-unknown-unknown`, where the frontend
//! uses it as a source of types. The parts that cannot exist there — an HTTP
//! client, the OS keyring — sit behind the default `native` feature.

pub mod chat;
pub mod config;
pub mod event;
pub mod implementation;
pub mod keystore;
pub mod logging;
pub mod repository;
pub mod tree;

#[derive(Debug)]
pub enum Persona {
    Human,
    Epik,
}
