//! The Epik window.
//!
//! This crate is a host and nothing more: it owns the window, the state
//! behind a mutex, the commands the frontend calls, and the pump that
//! forwards library events to it. Every capability it exposes must be
//! reachable through `epik` library calls alone — that invariant is what
//! keeps the daemon, the worker, and any future thin client possible.

// A GUI binary should not open a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> tauri::Result<()> {
    tauri::Builder::default().run(tauri::generate_context!())
}
