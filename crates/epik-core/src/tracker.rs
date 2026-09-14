//! The tracker seam: the system of record for issues, and the order
//! among them.
//!
//! An [`IssueTracker`] answers for issues and nothing else: what each
//! one says, where it stands, and which issues wait on which. The
//! order comes out as a [`Dag`] of ids, which is what a plan is built
//! from. `json`, behind the testing feature, is a tracker read from one
//! file, for tests.

#[cfg(feature = "testing")]
pub mod json;

use anyhow::Result;

use crate::dag::Dag;

/// What names an issue in its tracker.
pub type IssueId = String;

/// An issue tracker, its statuses of type `S`.
pub trait IssueTracker<S> {
    /// The issue's content: what implementing it means, in the words
    /// the tracker holds.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn content(&self, id: &str) -> Result<String>;

    /// Where the issue stands.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn get_status(&self, id: &str) -> Result<S>;

    /// Moves the issue to `status`.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn set_status(&mut self, id: &str, status: S) -> Result<()>;

    /// `root` and every issue that depends on it, transitively, each
    /// after the issues it depends on.
    ///
    /// # Errors
    ///
    /// No such issue, or the tracker's own failure.
    fn dependents(&self, root: &str) -> Result<Dag<IssueId>>;
}
