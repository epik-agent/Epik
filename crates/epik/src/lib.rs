pub mod agent;
pub mod chat;
#[cfg(feature = "serde")]
pub mod config;
#[cfg(all(feature = "native", unix))]
pub mod job;
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
// The vocabulary and the fold compile everywhere, like `feature`; only
// the `Log` inside needs the host.
pub mod monitor;
#[cfg(feature = "native")]
mod temp;
#[cfg(all(feature = "native", any(test, feature = "testing")))]
pub mod testing;
#[cfg(feature = "serde")]
#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub mod tools;
pub mod tracker;
