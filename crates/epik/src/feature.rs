//! Building a feature: a build made of builds.
//!
//! This slice carries [`merge`], the machinery by which work lands on a
//! feature branch. The plan — the tree of issues and the edges over it —
//! and the feature build that drives dispatch arrive in slices of their
//! own.

#[cfg(all(feature = "native", unix))]
pub mod merge;
