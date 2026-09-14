#![allow(dead_code)]
//! Agents: the Processes that write code, and the typed reading
//! of what each one says. [`claude`] is the first.

#[cfg(feature = "native")]
use crate::process::Exit;

pub mod claude;

#[cfg(feature = "native")]
trait Agent {
    fn new(prompt: String) -> Self;
    fn run() -> Exit;
}
