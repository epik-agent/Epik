//! The tracker seam: the system of record for issues.
//!
//! Issues, containment and ordering edges, comments, closing — that is a
//! tracker, and all a tracker is. Branches, pushes, and pull requests are
//! a forge's, and no verb for them appears here: the seam is cut along
//! that fault line, so a tracker answers for issues and nothing else,
//! whichever system happens to hold them.
//!
//! Failure is words. A tracker's callers are tool handlers, and the model
//! is who acts on a refusal, so every implementer renders its own errors
//! into a sentence a model reads.

#[cfg(feature = "native")]
pub mod github;

use crate::feature::{Feature, IssueId, Plan};

/// The issue verbs, and only the issue verbs.
pub trait Tracker {
    /// A feature's shape, read whole: the containment tree rooted at
    /// `feature`, the blocked-by edges over it, and any blocker pointing
    /// outside the tree.
    ///
    /// # Errors
    ///
    /// The tracker's failure in words a model reads.
    fn plan(&self, feature: &Feature) -> Result<Plan, String>;

    /// Leaves a comment on an issue.
    ///
    /// # Errors
    ///
    /// The tracker's failure in words a model reads.
    fn note(&self, issue: &IssueId, body: &str) -> Result<(), String>;

    /// Closes an issue. Epik closes only the leaves it built: a
    /// container's doneness is derived from its children and never
    /// written back, so no node ever has two sources of truth.
    ///
    /// # Errors
    ///
    /// The tracker's failure in words a model reads.
    fn close(&self, issue: &IssueId) -> Result<(), String>;
}
