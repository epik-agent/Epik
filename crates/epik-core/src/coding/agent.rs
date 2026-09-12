#![allow(dead_code)]
//! Coding agents: the Agents that write code, and the typed reading
//! of what each one says. [`claude`] is the first.

#[cfg(feature = "native")]
use crate::agent::Exit;

pub mod claude;

#[cfg(feature = "native")]
trait CodingAgent {
    fn new(prompt: String) -> Self;
    fn run() -> Exit;
}
