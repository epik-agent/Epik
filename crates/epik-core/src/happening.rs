//! What a task has to tell, and the names it is told under: the
//! vocabulary of the stream, spoken by a build and folded by a
//! watcher.
//!
//! The words live here rather than beside the machinery that speaks
//! them because the two halves compile differently. A build needs a
//! runtime, processes and git; the [`monitor`](crate::monitor)'s fold
//! needs none of that and runs in a browser. Defining the words below
//! that line lets the fold read them without dragging the machinery
//! in, and [`plan::stream`](crate::plan::stream) and
//! [`plan::feature`](crate::plan::feature) re-export them, so they are
//! still spelled where they are spoken.

use serde::{Deserialize, Serialize};

use crate::agent::Update;

/// Something that happened at `node`, which is whatever the plan names
/// its nodes by.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Event<N> {
    pub node: N,
    pub happened: Happened,
}

/// What a task has to tell: its lifecycle, and between the start and
/// the end, what its process said.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Happened {
    /// The task began to run.
    Started,
    /// An agent said this on stdout, as its engine's protocol reads.
    Said(Update),
    /// A line of the process's stderr: where a CLI's last words land.
    Stderr(String),
    /// The commit the issue's branch was pushed at. The one thing a
    /// step says of what it produced, because it is the commit every
    /// step after it names — the pull request's head, the checks'
    /// subject, the merge's argument — and the one a merged issue is
    /// remembered by.
    Pushed(String),
    /// What the wait for the required checks came to. The verdict is
    /// the issue's own fact, so whether its merge was checked is read
    /// here rather than inferred from the build it belongs to.
    Checked(Verdict),
    /// The conclusions of the check runs the policy leaves out, named
    /// and read together. They decide nothing, and a build that waited
    /// on the required checks alone still says what the rest came to.
    Ran(String),
    /// The task succeeded.
    Finished,
    /// The task failed, in the error's own words, causes and all.
    Failed(String),
}

/// What a wait saw the required checks come to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Verdict {
    /// Every required check concluded green.
    Green,
    /// A required check concluded red: the names of those that did.
    Red(String),
    /// A policy requiring no check: the merge proceeds and claims
    /// nothing.
    Unclaimed,
}

/// What a feature build's events are filed under: a node of its plan,
/// or one step of an issue's node.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Name {
    Branch,
    Issue(u64),
    Step(u64, Step),
    PullRequest,
}

/// The steps of an issue's node, in the order they run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Step {
    /// The issue's branch cut at the feature branch's tip, in a
    /// worktree of its own.
    Worktree,
    /// The agent's run there.
    Agent,
    /// What the agent left, committed as Epik.
    Commit,
    /// The issue's branch pushed.
    Push,
    /// The issue's pull request opened into the feature branch.
    PullRequest,
    /// The wait for the pushed commit's checks.
    Checks,
    /// The pull request merged, at the commit its checks judged.
    Merge,
    /// The issue closed.
    Close,
}
