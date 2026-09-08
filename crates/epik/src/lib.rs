pub mod agent;
pub mod chat;
#[cfg(all(feature = "native", unix))]
pub mod job;
#[cfg(feature = "native")]
pub use agent::child::spawn;
#[cfg(feature = "serde")]
pub mod config;
// Compiled without `native` so the vocabulary stays wasm-clean, though
// nothing but native code calls into it yet — likewise `github` and
// `tools`.
#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub mod feature;
#[cfg(all(feature = "native", unix))]
pub mod forge;
#[cfg(feature = "native")]
pub mod git;
#[cfg(feature = "serde")]
#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub mod github;
pub mod keystore;
#[cfg(feature = "native")]
mod temp;
#[cfg(all(feature = "native", any(test, feature = "testing")))]
pub mod testing;
#[cfg(feature = "serde")]
#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub mod tools;
pub mod tracker;
