// Moved to epik-core; re-exported here until the rest follows.
pub use epik_core::github;
#[cfg(feature = "native")]
pub use epik_core::process;
pub mod chat;
#[cfg(feature = "serde")]
pub mod config;
#[cfg(all(feature = "native", unix))]
pub mod job;
// Compiled without `native` so the vocabulary stays wasm-clean, though
// nothing but native code calls into it yet — likewise `tools`.
#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub mod feature;
#[cfg(all(feature = "native", unix))]
pub mod forge;
#[cfg(feature = "native")]
pub mod git;
pub mod keystore;
// The vocabulary and the fold compile everywhere, like `feature`; only
// the `Log` inside needs the host.
pub mod monitor;
#[cfg(feature = "native")]
use epik_core::temp;
#[cfg(all(feature = "native", any(test, feature = "testing")))]
pub mod testing;
#[cfg(feature = "serde")]
#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub mod tools;
pub mod tracker;
