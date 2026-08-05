//! The Epik window.
//!
//! This crate is a host and nothing more: it owns the window, the state
//! behind a mutex, the commands the frontend calls, and the pump that
//! forwards library events to it. Every capability it exposes must be
//! reachable through `epik` library calls alone — that invariant is what
//! keeps the daemon, the worker, and any future thin client possible.
//!
//! A note on the content-security policy in `tauri.conf.json`, which is
//! stricter than Tauri's default and so has to name two things a JavaScript
//! frontend would never ask for: `'wasm-unsafe-eval'` in `script-src`, without
//! which the webview refuses to instantiate the Leptos module at all, and
//! `ipc:` with `http://ipc.localhost` in `connect-src`, which is how Tauri's
//! own command channel reaches this process.

// A GUI binary should not open a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod host;

use std::sync::mpsc;

use tauri::Manager;

use host::Window;

fn main() -> tauri::Result<()> {
    tauri::Builder::default()
        .setup(|app| {
            // The library emits into the sender; the pump drains the receiver
            // into the webview. Nothing in between knows about both.
            let (events, incoming) = mpsc::channel();
            app.manage(Window::open(events));
            host::pump(app.handle().clone(), incoming);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            host::status,
            host::send_message,
            host::set_api_key,
        ])
        .run(tauri::generate_context!())
}
