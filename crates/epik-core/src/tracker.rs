//! The tracker seam: the system of record for issues, and the shape
//! among them.
//!
//! An [`IssueTracker`] answers for issues and nothing else: what each
//! one says, where it stands, which issues contain which, and which
//! wait on which. An id is whatever names an issue in that tracker — a
//! number on GitHub, a key like `EPK-12` elsewhere — and the shape
//! comes out as a [`Dag`] of them, which is what a plan is built from.
//! [`github`] is GitHub as one.
//!
//! Two relationships give the shape, each meaning what a tracker means
//! by it. A sub-issue edge is containment: the issues under a root are
//! its members, and the members holding no sub-issues are the work. A
//! blocked-by edge is order. [`work`] folds the two into one graph, one
//! [`Member`] at a time.

#[cfg(feature = "native")]
pub mod github;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use anyhow::{Result, bail};

use crate::dag::{Dag, Node};

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

    /// The work under `root`, as [`work`] folds it: the open issues
    /// `root` contains that contain none themselves, each after the
    /// work blocking it.
    ///
    /// # Errors
    ///
    /// No such issue, an open blocker outside `root`, or the tracker's
    /// own failure.
    fn work(&self, root: &I) -> Result<Dag<I>>;
}

/// An issue at the far end of an edge, and whether it is open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Related<I> {
    pub id: I,
    pub open: bool,
}

/// An issue as the shape of a plan reads it: whether it is open, the
/// issues it contains, and the issues it waits on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Member<I> {
    pub open: bool,
    pub sub_issues: Vec<Related<I>>,
    pub blocked_by: Vec<Related<I>>,
}

/// The work under `root`: the leaves of its open sub-issue tree, in
/// the tree's order, each after the leaves blocking it. Each open
/// member is read by `fetch` once.
///
/// `root` and every member holding sub-issues are containers: structure
/// around the work. A blocked-by edge onto a container is inherited by
/// every leaf beneath it, and a container named as a blocker stands
/// for every leaf beneath it. A closed issue is finished: as a member
/// it goes unread, with everything beneath it, and as a blocker it is
/// satisfied.
///
/// # Errors
///
/// A fetch that fails; an issue contained twice; an open blocker
/// outside the members, in words naming it; or blocked-by edges that
/// run in a cycle.
pub fn work<I: Clone + Display + Ord>(
    root: &I,
    fetch: impl Fn(&I) -> Result<Member<I>>,
) -> Result<Dag<I>> {
    let mut read = BTreeMap::new();
    let mut unread = vec![root.clone()];
    while let Some(id) = unread.pop() {
        if read.contains_key(&id) {
            bail!("issue {id} is contained twice under issue {root}");
        }
        let member = fetch(&id)?;
        unread.extend(open(&member.sub_issues).cloned());
        read.insert(id, member);
    }
    let mut work = Vec::new();
    Members { root, read }.descend(root, &[], &mut work)?;
    let positions: BTreeMap<&I, usize> = work.iter().map(|(id, _)| id).zip(0..).collect();
    Dag::new(
        work.iter()
            .map(|(id, blockers)| {
                let before: BTreeSet<usize> = blockers.iter().map(|id| positions[id]).collect();
                Node::new(id.clone(), before)
            })
            .collect(),
    )
}

/// The ids of the open issues among `related`.
fn open<I>(related: &[Related<I>]) -> impl Iterator<Item = &I> {
    related
        .iter()
        .filter(|issue| issue.open)
        .map(|issue| &issue.id)
}

/// The members under `root`, each as read: `root`, and every open
/// sub-issue of a member. A tree by containment.
struct Members<'a, I> {
    root: &'a I,
    read: BTreeMap<I, Member<I>>,
}

impl<I: Clone + Display + Ord> Members<'_, I> {
    /// Adds to `work` each leaf at or beneath `id`, in the tree's
    /// order, with the leaves blocking it: those `inherited` from the
    /// containers above, and its own.
    fn descend(&self, id: &I, inherited: &[I], work: &mut Vec<(I, Vec<I>)>) -> Result<()> {
        let member = &self.read[id];
        let mut blockers = inherited.to_vec();
        for blocker in open(&member.blocked_by) {
            if !self.read.contains_key(blocker) {
                bail!(
                    "issue {id} is blocked by issue {blocker}, \
                     which is open and outside issue {}",
                    self.root
                );
            }
            blockers.extend(self.leaves(blocker));
        }
        if member.sub_issues.is_empty() {
            if member.open {
                work.push((id.clone(), blockers));
            }
            return Ok(());
        }
        open(&member.sub_issues).try_for_each(|id| self.descend(id, &blockers, work))
    }

    /// The leaves at or beneath `id`, in the tree's order.
    fn leaves(&self, id: &I) -> Vec<I> {
        let member = &self.read[id];
        if member.sub_issues.is_empty() {
            return member.open.then(|| id.clone()).into_iter().collect();
        }
        open(&member.sub_issues)
            .flat_map(|id| self.leaves(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::RefCell;

    /// A tracker's worth of issues — each open or closed, with its
    /// sub-issues and its blockers — and the ids read from it.
    #[derive(Default)]
    struct Issues {
        issues: BTreeMap<u64, (bool, Vec<u64>, Vec<u64>)>,
        reads: RefCell<Vec<u64>>,
    }

    impl Issues {
        /// With `id` open, containing `sub_issues` and waiting on
        /// `blocked_by`.
        fn open(mut self, id: u64, sub_issues: &[u64], blocked_by: &[u64]) -> Self {
            self.issues
                .insert(id, (true, sub_issues.to_vec(), blocked_by.to_vec()));
            self
        }

        /// With `id` closed, containing `sub_issues`.
        fn closed(mut self, id: u64, sub_issues: &[u64]) -> Self {
            self.issues.insert(id, (false, sub_issues.to_vec(), vec![]));
            self
        }

        fn related(&self, ids: &[u64]) -> Vec<Related<u64>> {
            ids.iter()
                .map(|&id| Related {
                    id,
                    open: self.issues[&id].0,
                })
                .collect()
        }

        fn work(&self, root: u64) -> Result<Dag<u64>> {
            work(&root, |id| {
                self.reads.borrow_mut().push(*id);
                let (open, sub_issues, blocked_by) = self
                    .issues
                    .get(id)
                    .ok_or_else(|| anyhow!("no issue {id}"))?;
                Ok(Member {
                    open: *open,
                    sub_issues: self.related(sub_issues),
                    blocked_by: self.related(blocked_by),
                })
            })
        }

        /// The ids read, sorted.
        fn reads(self) -> Vec<u64> {
            let mut reads = self.reads.into_inner();
            reads.sort_unstable();
            reads
        }
    }

    /// Each work node with the nodes directly before it.
    fn shape(dag: &Dag<u64>) -> Vec<(u64, Vec<u64>)> {
        dag.nodes()
            .map(|node| {
                let before = node.predecessors().map(|before| *before.value());
                (*node.value(), before.collect())
            })
            .collect()
    }

    /// #158 as it was filed: four sub-issues, the order among them in
    /// blocked-by edges.
    fn feature_158() -> Issues {
        Issues::default()
            .open(158, &[154, 155, 156, 157], &[])
            .open(154, &[], &[])
            .open(155, &[], &[])
            .open(156, &[], &[154, 155])
            .open(157, &[], &[156])
    }

    #[test]
    fn the_sub_issues_of_a_feature_are_its_work_in_blocked_by_order() {
        let dag = feature_158().work(158).unwrap();
        assert_eq!(
            shape(&dag),
            [
                (154, vec![]),
                (155, vec![]),
                (156, vec![154, 155]),
                (157, vec![156])
            ]
        );
    }

    #[test]
    fn each_member_is_read_once() {
        let issues = feature_158();
        issues.work(158).unwrap();
        assert_eq!(issues.reads(), [154, 155, 156, 157, 158]);
    }

    #[test]
    fn an_issue_holding_no_sub_issues_is_its_own_work() {
        let issues = Issues::default().open(7, &[], &[]);
        assert_eq!(shape(&issues.work(7).unwrap()), [(7, vec![])]);
        assert_eq!(issues.reads(), [7]);
    }

    #[test]
    fn a_nested_container_contributes_its_leaves_in_the_trees_order() {
        let issues = Issues::default()
            .open(1, &[2, 3, 6], &[])
            .open(2, &[], &[])
            .open(3, &[4, 5], &[])
            .open(4, &[], &[])
            .open(5, &[], &[])
            .open(6, &[], &[]);
        assert_eq!(
            shape(&issues.work(1).unwrap()),
            [(2, vec![]), (4, vec![]), (5, vec![]), (6, vec![])]
        );
    }

    #[test]
    fn an_edge_onto_a_container_is_inherited_by_each_leaf_beneath_it() {
        // The container 3 waits on 2, and so do 4 and the nested 6.
        let issues = Issues::default()
            .open(1, &[2, 3], &[])
            .open(2, &[], &[])
            .open(3, &[4, 5], &[2])
            .open(4, &[], &[])
            .open(5, &[6], &[])
            .open(6, &[], &[4]);
        assert_eq!(
            shape(&issues.work(1).unwrap()),
            [(2, vec![]), (4, vec![2]), (6, vec![2, 4])]
        );
    }

    #[test]
    fn a_container_as_blocker_stands_for_each_leaf_beneath_it() {
        let issues = Issues::default()
            .open(1, &[2, 5], &[])
            .open(2, &[3, 4], &[])
            .open(3, &[], &[])
            .open(4, &[], &[])
            .open(5, &[], &[2]);
        assert_eq!(
            shape(&issues.work(1).unwrap()),
            [(3, vec![]), (4, vec![]), (5, vec![3, 4])]
        );
    }

    #[test]
    fn an_open_blocker_outside_the_members_refuses_the_plan_and_is_named() {
        let issues = Issues::default()
            .open(1, &[2], &[])
            .open(2, &[], &[9])
            .open(9, &[], &[]);
        assert_eq!(
            issues.work(1).unwrap_err().to_string(),
            "issue 2 is blocked by issue 9, which is open and outside issue 1"
        );
    }

    #[test]
    fn a_closed_blocker_outside_the_members_is_satisfied_unread() {
        let issues = Issues::default()
            .open(1, &[2], &[])
            .open(2, &[], &[9])
            .closed(9, &[]);
        assert_eq!(shape(&issues.work(1).unwrap()), [(2, vec![])]);
        assert_eq!(issues.reads(), [1, 2]);
    }

    #[test]
    fn a_closed_member_is_finished_work_and_satisfies_the_edges_naming_it() {
        // 2 is closed, and so is the container 4 with all beneath it.
        let issues = Issues::default()
            .open(1, &[2, 3, 4], &[])
            .closed(2, &[])
            .open(3, &[], &[2, 4])
            .closed(4, &[5])
            .open(5, &[], &[]);
        assert_eq!(shape(&issues.work(1).unwrap()), [(3, vec![])]);
        assert_eq!(issues.reads(), [1, 3]);
    }

    #[test]
    fn a_closed_root_holding_no_sub_issues_is_no_work() {
        let issues = Issues::default().closed(7, &[]);
        assert!(shape(&issues.work(7).unwrap()).is_empty());
    }

    #[test]
    fn an_issue_contained_twice_is_refused() {
        let issues = Issues::default()
            .open(1, &[2, 3], &[])
            .open(2, &[3], &[])
            .open(3, &[], &[]);
        assert_eq!(
            issues.work(1).unwrap_err().to_string(),
            "issue 3 is contained twice under issue 1"
        );
    }

    #[test]
    fn a_fetch_that_fails_is_the_works_failure() {
        let error = work(&1, |_| Err(anyhow!("offline"))).unwrap_err();
        assert_eq!(error.to_string(), "offline");
    }

    #[test]
    fn blocked_by_edges_in_a_cycle_are_refused() {
        let issues = Issues::default()
            .open(1, &[2, 3], &[])
            .open(2, &[], &[3])
            .open(3, &[], &[2]);
        let error = issues.work(1).unwrap_err().to_string();
        assert!(error.contains("cycle"), "{error}");
    }
}
