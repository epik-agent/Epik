//! The tracker seam: the system of record for issues, and the order
//! among them.
//!
//! An [`IssueTracker`] answers for issues and nothing else: what each
//! one says, where it stands, and which issues wait on which. An id is
//! whatever names an issue in that tracker — a number on GitHub, a key
//! like `EPK-12` elsewhere — and the order comes out as a [`Dag`] of
//! them, which is what a plan is built from. [`github`] is GitHub as
//! one.

#[cfg(feature = "native")]
pub mod github;

use anyhow::Result;

use crate::dag::Dag;

/// An issue tracker: its ids of type `I`, its statuses of type `S`.
pub trait IssueTracker<I, S> {
    /// The issue's content: what implementing it means, in the words
    /// the tracker holds.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn content(&self, id: &I) -> Result<String>;

    /// Where the issue stands.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn get_status(&self, id: &I) -> Result<S>;

    /// Moves the issue to `status`.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn set_status(&mut self, id: &I, status: S) -> Result<()>;

    /// `root` and every issue that depends on it, transitively, each
    /// after the issues it depends on.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn dependents(&self, root: &I) -> Result<Dag<I>>;
}
