//! A feature's shape: the plan, before anything builds.
//!
//! A feature is an issue with issues beneath it. Containment nests — a
//! feature may hold sub-features — and ordering does not, so a [`Plan`]
//! carries the two separately over the same nodes: a [`Tree`] of
//! [`Issue`]s rooted at the feature issue, a flat list of [`Blocking`]
//! edges, and whatever blockers point outside the tree, each recorded
//! from the edge itself with the state that decides whether it holds.
//!
//! Sub-issue semantics are stated once, in [`Plan::settled`]: an issue is
//! settled when it is in the done set, or closed in the tracker, or a
//! container whose every child is settled. [`Plan::ready`] is the same
//! predicate applied three ways — false for a leaf, false for each of its
//! ancestors, true for each of its blockers — so a closed container stops
//! its children without a rule of its own. [`Plan::problems`] names what
//! is wrong with a plan's shape rather than scheduling around it.
//!
//! Reading a plan out of a tracker is [`Plan::descend`], a pure function
//! over an injected fetch closure; the [`Tracker`](crate::tracker::Tracker)
//! seam is where a real tracker plugs in. Everything here compiles with no
//! features enabled: the vocabulary is the library's, not any provider's;
//! the machinery is gated — the `merge` module by which work lands on a
//! feature branch, the build that folds a whole plan into work —
//! [`build`], the verb, and [`Build`], its record — and the `tools`
//! module through which a chat window starts a feature build and reads
//! how it is going.

#[cfg(all(feature = "native", unix))]
mod build;
#[cfg(all(feature = "native", unix))]
pub mod merge;
#[cfg(all(feature = "native", unix))]
pub mod tools;

#[cfg(all(feature = "native", unix))]
pub use build::{Budget, Build, CONCURRENCY, Slot, State, build};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Serialize, Serializer};

use crate::tree::Tree;

/// A tracker's name for an issue. A string, because a Linear key is not a
/// number; GitHub's numbers ride in as their decimal spelling.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct IssueId(pub String);

impl fmt::Display for IssueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for IssueId {
    fn from(id: &str) -> Self {
        Self(id.to_owned())
    }
}

impl From<u64> for IssueId {
    fn from(number: u64) -> Self {
        Self(number.to_string())
    }
}

/// An issue as a plan carries one: enough to schedule and render — title
/// and state, no body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Issue {
    pub id: IssueId,
    pub title: String,
    pub closed: bool,
}

/// One ordering edge: `issue` waits until `blocker` settles. Edges cross
/// the containment tree freely, and a blocker may be a container — "the
/// docs wait on the whole API" is the natural thing to say — or an issue
/// outside the tree altogether.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Blocking {
    pub issue: IssueId,
    pub blocker: IssueId,
}

/// One issue and the edges around it, as one fetch answers during the
/// descent: the sub-issues it contains, by id — each is fetched in its
/// own turn — and the issues blocking it, whole, because a blocker
/// outside the tree is never fetched and must carry its state on the
/// edge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub issue: Issue,
    pub children: Vec<IssueId>,
    pub blockers: Vec<Issue>,
}

/// What can be wrong with a plan's shape, named rather than scheduled
/// around. Serialized — a tool result on its way to a model — as its
/// [`Display`](fmt::Display) words.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Problem {
    /// Blocked-by edges that chase each other's tails: the ids around the
    /// loop, first one repeated at the end. Nothing on a cycle can ever
    /// start.
    Cycle(Vec<IssueId>),
    /// An edge with an end in neither the tree nor the outside blockers:
    /// the plan cannot say whether it holds.
    Dangling(Blocking),
    /// Every leaf is settled before anything begins: the plan has no work
    /// in it.
    NoWork,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle(ids) => {
                let loop_ = ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" -> ");
                write!(f, "blocked-by edges form a cycle: {loop_}")
            }
            Self::Dangling(edge) => write!(
                f,
                "the edge \"{} is blocked by {}\" points at an issue the plan does not know",
                edge.issue, edge.blocker
            ),
            Self::NoWork => write!(
                f,
                "every issue is already settled: the plan has no work in it"
            ),
        }
    }
}

impl Serialize for Problem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The most issues one descent may gather before it refuses. GitHub caps
/// sub-issues at one hundred per parent; a feature past this cap is not a
/// feature anyone should build in one breath.
const CAP: usize = 500;

/// A feature's plan: containment as a tree, ordering as a flat edge list
/// over the same ids, and the blockers that point outside the tree.
/// Output vocabulary, like everything here: serialized on its way to a
/// model, never read back in.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Plan {
    /// The feature issue at the root, its decomposition beneath it.
    /// Interior nodes are containers, not work; only leaves get an Agent.
    pub tree: Tree<Issue>,
    /// The blocked-by edges, every `issue` an id in the tree.
    pub blocking: Vec<Blocking>,
    /// Blockers outside the tree, recorded from the edges that name them,
    /// each carrying the state that decides whether it holds.
    pub outside: Vec<Issue>,
}

impl Plan {
    /// The descent from a feature issue to a plan. `fetch` answers for one
    /// issue at a time — a pure function over that closure, so fixtures
    /// exercise it with no network. A visited set keeps any graph finite
    /// (an issue reached twice keeps its first place in the tree), and a
    /// node cap makes exhaustion a refusal in words rather than a
    /// truncated plan that reads as complete.
    ///
    /// # Errors
    ///
    /// Whatever `fetch` fails with, passed through; or the cap's refusal.
    pub fn descend(
        feature: &IssueId,
        mut fetch: impl FnMut(&IssueId) -> Result<Node, String>,
    ) -> Result<Self, String> {
        let mut visited = BTreeSet::new();
        let mut blocking = Vec::new();
        let mut blockers = BTreeMap::new();
        let tree = gather(
            feature,
            &mut fetch,
            &mut visited,
            &mut blocking,
            &mut blockers,
        )?;
        let outside = blockers
            .into_values()
            .filter(|issue: &Issue| !visited.contains(&issue.id))
            .collect();
        Ok(Self {
            tree,
            blocking,
            outside,
        })
    }

    /// The one place containment is interpreted: an issue is settled when
    /// it is in the done set, or closed in the tracker, or a container
    /// whose every child is settled. An id the plan does not know is never
    /// settled — and [`problems`](Self::problems) names the edge that
    /// asked.
    #[must_use]
    pub fn settled(&self, id: &IssueId, done: &BTreeSet<IssueId>) -> bool {
        done.contains(id)
            || self.tree.find_path(|issue| &issue.id == id).map_or_else(
                || {
                    self.outside
                        .iter()
                        .any(|issue| &issue.id == id && issue.closed)
                },
                |path| settled(path.last().expect("a found path reaches its match"), done),
            )
    }

    /// The plan's work: the open leaves under no settled ancestor —
    /// everything a build could run, now or later. An abandoned subtree
    /// needs no rule of its own; it is the ancestor clause.
    #[must_use]
    pub fn work(&self, done: &BTreeSet<IssueId>) -> Vec<&Issue> {
        self.tree
            .leaves()
            .filter(|leaf| {
                self.tree
                    .find_path(|issue| issue.id == leaf.id)
                    .expect("a leaf is in its own tree")
                    .iter()
                    .all(|node| !settled(node, done))
            })
            .collect()
    }

    /// The leaves ready to start: the [`work`](Self::work), kept where
    /// [`settled`](Self::settled) is true for every blocker — the same
    /// predicate applied three ways.
    #[must_use]
    pub fn ready(&self, done: &BTreeSet<IssueId>) -> Vec<&Issue> {
        let blockers = self.blockers();
        self.work(done)
            .into_iter()
            .filter(|leaf| {
                blockers
                    .get(&leaf.id)
                    .into_iter()
                    .flatten()
                    .all(|blocker| self.settled(blocker, done))
            })
            .collect()
    }

    /// The work a build can no longer reach: `done` is what has landed,
    /// `lost` the leaves that will never settle — failed issues, chiefly
    /// — and the answer maps each open leaf that can never become ready
    /// to the first blocker it is stuck on. A blocker never settles when
    /// it is lost, a container holding a lost or stuck leaf, an open
    /// issue outside the tree — within one build, permanent, since
    /// nothing here closes outside issues — an id the plan does not
    /// know, or a companion on a blocked-by cycle. Computed as a growth
    /// fixpoint over the leaves that still can land, so mutual blocking
    /// dooms both sides rather than propping them up.
    #[must_use]
    pub fn doomed(
        &self,
        done: &BTreeSet<IssueId>,
        lost: &BTreeSet<IssueId>,
    ) -> BTreeMap<IssueId, IssueId> {
        let blockers = self.blockers();
        let work = self.work(done);
        let mut alive: BTreeSet<&IssueId> = BTreeSet::new();
        loop {
            let grown: Vec<&IssueId> = work
                .iter()
                .filter(|leaf| !alive.contains(&leaf.id) && !lost.contains(&leaf.id))
                .filter(|leaf| {
                    blockers
                        .get(&leaf.id)
                        .into_iter()
                        .flatten()
                        .all(|blocker| self.settles(blocker, done, &alive))
                })
                .map(|leaf| &leaf.id)
                .collect();
            if grown.is_empty() {
                break;
            }
            alive.extend(grown);
        }
        work.iter()
            .filter(|leaf| !lost.contains(&leaf.id) && !alive.contains(&leaf.id))
            .filter_map(|leaf| {
                blockers
                    .get(&leaf.id)
                    .into_iter()
                    .flatten()
                    .find(|blocker| !self.settles(blocker, done, &alive))
                    .map(|blocker| (leaf.id.clone(), (*blocker).clone()))
            })
            .collect()
    }

    /// Whether a build can ever count `id` settled, given the `alive`
    /// leaves it will still run: already done or closed — an outside
    /// issue by the state its edge recorded — or a tree node each of
    /// whose leaves is alive or already covered by a done or closed
    /// step. An id the plan does not know never settles, which is also
    /// what [`problems`](Self::problems) says of its edge.
    fn settles(&self, id: &IssueId, done: &BTreeSet<IssueId>, alive: &BTreeSet<&IssueId>) -> bool {
        done.contains(id)
            || match self.tree.find_path(|issue| &issue.id == id) {
                None => self
                    .outside
                    .iter()
                    .any(|issue| &issue.id == id && issue.closed),
                Some(path) => {
                    let node = path.last().expect("a found path reaches its match");
                    node.leaves().all(|leaf| {
                        alive.contains(&leaf.id)
                            || node
                                .find_path(|issue| issue.id == leaf.id)
                                .expect("a leaf is in its own subtree")
                                .iter()
                                .any(|step| done.contains(&step.value.id) || step.value.closed)
                    })
                }
            }
    }

    /// Everything wrong with the plan's shape: cycles among the blocked-by
    /// edges, edges pointing at issues in neither the tree nor the outside
    /// set, and a plan with no work in it. Terminates on any graph — a
    /// broken plan is precisely when this answer matters.
    #[must_use]
    pub fn problems(&self) -> Vec<Problem> {
        let known: BTreeSet<&IssueId> = self
            .tree
            .nodes()
            .chain(self.outside.iter())
            .map(|issue| &issue.id)
            .collect();
        let mut problems: Vec<Problem> = self
            .blocking
            .iter()
            .filter(|edge| !known.contains(&edge.issue) || !known.contains(&edge.blocker))
            .cloned()
            .map(Problem::Dangling)
            .collect();
        problems.extend(self.cycles().into_iter().map(Problem::Cycle));
        if settled(&self.tree, &BTreeSet::new()) {
            problems.push(Problem::NoWork);
        }
        problems
    }

    /// The blocked-by edges indexed by their `issue` side, built once per
    /// query so no caller rescans the whole edge list per node.
    fn blockers(&self) -> BTreeMap<&IssueId, Vec<&IssueId>> {
        let mut index: BTreeMap<&IssueId, Vec<&IssueId>> = BTreeMap::new();
        for edge in &self.blocking {
            index.entry(&edge.issue).or_default().push(&edge.blocker);
        }
        index
    }

    /// Every cycle among the blocked-by edges, each found once: a
    /// depth-first walk that finishes each id exactly once, so it ends on
    /// any graph.
    fn cycles(&self) -> Vec<Vec<IssueId>> {
        let blockers = self.blockers();
        let mut cycles = Vec::new();
        let mut finished = BTreeSet::new();
        let mut path = Vec::new();
        for start in self.tree.nodes().map(|issue| &issue.id) {
            chase(start, &blockers, &mut path, &mut finished, &mut cycles);
        }
        cycles
    }
}

/// One step of the cycle hunt: an id already on the path closes a loop;
/// an id already finished has told everything it knows.
fn chase<'a>(
    id: &'a IssueId,
    blockers: &BTreeMap<&'a IssueId, Vec<&'a IssueId>>,
    path: &mut Vec<&'a IssueId>,
    finished: &mut BTreeSet<&'a IssueId>,
    cycles: &mut Vec<Vec<IssueId>>,
) {
    if finished.contains(id) {
        return;
    }
    if let Some(entered) = path.iter().position(|seen| *seen == id) {
        let mut cycle: Vec<IssueId> = path[entered..].iter().copied().cloned().collect();
        cycle.push(id.clone());
        cycles.push(cycle);
        return;
    }
    path.push(id);
    for blocker in blockers.get(id).into_iter().flatten() {
        chase(blocker, blockers, path, finished, cycles);
    }
    path.pop();
    finished.insert(id);
}

/// [`Plan::settled`], said of a subtree: every chain from this node down
/// to one of its leaves passes through an issue that is done or closed.
/// That is the recursive rule — done, or closed in the tracker, or a
/// container whose every child is settled — unrolled onto the one
/// navigation primitive, so containment is read and never re-derived.
fn settled(node: &Tree<Issue>, done: &BTreeSet<IssueId>) -> bool {
    node.leaves().all(|leaf| {
        node.find_path(|issue| issue.id == leaf.id)
            .expect("a leaf is in its own subtree")
            .iter()
            .any(|step| done.contains(&step.value.id) || step.value.closed)
    })
}

/// One step of the descent: fetch `id`, record the edges around it, and
/// recurse into the children not yet seen.
fn gather(
    id: &IssueId,
    fetch: &mut impl FnMut(&IssueId) -> Result<Node, String>,
    visited: &mut BTreeSet<IssueId>,
    blocking: &mut Vec<Blocking>,
    blockers: &mut BTreeMap<IssueId, Issue>,
) -> Result<Tree<Issue>, String> {
    if visited.len() == CAP {
        return Err(format!(
            "this feature decomposes into more than {CAP} issues; \
             refusing to answer with a truncated plan that would read as complete"
        ));
    }
    visited.insert(id.clone());
    let node = fetch(id)?;
    for blocker in node.blockers {
        blocking.push(Blocking {
            issue: id.clone(),
            blocker: blocker.id.clone(),
        });
        blockers.insert(blocker.id.clone(), blocker);
    }
    let mut children = Vec::new();
    for child in &node.children {
        if !visited.contains(child) {
            children.push(gather(child, fetch, visited, blocking, blockers)?);
        }
    }
    Ok(Tree {
        value: node.issue,
        children,
    })
}

/// Plan builders shared by this module's tests and its submodules':
/// numbered issues, the nodes a fetch would answer with, and a descent
/// over them — no network, no tracker, just the graph a test states.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::{Issue, IssueId, Node, Plan};

    pub(crate) fn id(number: u64) -> IssueId {
        IssueId::from(number)
    }

    pub(crate) fn issue(number: u64, closed: bool) -> Issue {
        Issue {
            id: id(number),
            title: format!("issue {number}"),
            closed,
        }
    }

    /// One fixture node: the issue, who it contains, and who blocks it —
    /// blockers as (id, closed) pairs, exactly what an edge carries.
    pub(crate) fn node(
        number: u64,
        closed: bool,
        children: &[u64],
        blockers: &[(u64, bool)],
    ) -> Node {
        Node {
            issue: issue(number, closed),
            children: children.iter().copied().map(id).collect(),
            blockers: blockers
                .iter()
                .map(|&(number, closed)| issue(number, closed))
                .collect(),
        }
    }

    pub(crate) fn plan(feature: u64, nodes: Vec<Node>) -> Plan {
        Plan::descend(&id(feature), |asked| {
            nodes
                .iter()
                .find(|node| &node.issue.id == asked)
                .cloned()
                .ok_or_else(|| format!("no fixture for {asked}"))
        })
        .unwrap()
    }

    /// The smallest plan with an order in it: feature 1 holds 2 and 3,
    /// and 3 waits on 2. Two ready states, one edge — enough to watch
    /// one issue's end change another's standing.
    pub(crate) fn two_then_three() -> Plan {
        plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
            ],
        )
    }

    /// The same two, each waiting on the other: nothing can ever be
    /// ready, and the cycle is the plan's problem to name.
    pub(crate) fn two_and_three_in_a_cycle() -> Plan {
        plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[(3, false)]),
                node(3, false, &[], &[(2, false)]),
            ],
        )
    }

    /// A chain, 2 then 3 then 5, beside 4, which waits on nothing:
    /// what a loss at the head takes down, and what it spares.
    pub(crate) fn a_chain_beside_a_loner() -> Plan {
        plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
                node(4, false, &[], &[]),
                node(5, false, &[], &[(3, false)]),
            ],
        )
    }

    /// Feature 1 holds a container, 7, with leaves 2 and 8 — and a leaf
    /// of its own, 9, that waits on the container: what a loss inside 7
    /// means for whoever waited on 7 as a whole.
    pub(crate) fn nine_waiting_on_container_seven() -> Plan {
        plan(
            1,
            vec![
                node(1, false, &[7, 9], &[]),
                node(7, false, &[2, 8], &[]),
                node(2, false, &[], &[]),
                node(8, false, &[], &[]),
                node(9, false, &[], &[(7, false)]),
            ],
        )
    }

    /// Feature 7 with one leaf, 8: the least plan a tool can act on.
    pub(crate) fn seven_holding_eight() -> Plan {
        plan(7, vec![node(7, false, &[8], &[]), node(8, false, &[], &[])])
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{
        a_chain_beside_a_loner, id, issue, nine_waiting_on_container_seven, node, plan,
        two_and_three_in_a_cycle, two_then_three,
    };
    use super::*;

    fn ready_ids(plan: &Plan) -> Vec<&str> {
        plan.ready(&BTreeSet::new())
            .iter()
            .map(|issue| issue.id.0.as_str())
            .collect()
    }

    #[test]
    fn a_chain_readies_only_its_head() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2, 3, 4], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
                node(4, false, &[], &[(3, false)]),
            ],
        );
        assert_eq!(ready_ids(&plan), ["2"]);
        assert!(plan.problems().is_empty());
    }

    #[test]
    fn a_partly_closed_chain_readies_the_first_open_link() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2, 3, 4], &[]),
                node(2, true, &[], &[]),
                node(3, false, &[], &[(2, true)]),
                node(4, false, &[], &[(3, false)]),
            ],
        );
        assert_eq!(ready_ids(&plan), ["3"], "closed 2 releases 3, not 4");
    }

    #[test]
    fn a_diamond_readies_both_arms_once_the_top_lands() {
        let nodes = |top_closed: bool| {
            vec![
                node(1, false, &[2, 3, 4, 5], &[]),
                node(2, top_closed, &[], &[]),
                node(3, false, &[], &[(2, top_closed)]),
                node(4, false, &[], &[(2, top_closed)]),
                node(5, false, &[], &[(3, false), (4, false)]),
            ]
        };
        assert_eq!(ready_ids(&plan(1, nodes(false))), ["2"]);
        assert_eq!(
            ready_ids(&plan(1, nodes(true))),
            ["3", "4"],
            "both arms at once; the join still waits on both"
        );
    }

    #[test]
    fn independent_issues_are_all_ready_at_once() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2, 3, 4], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[]),
                node(4, false, &[], &[]),
            ],
        );
        assert_eq!(ready_ids(&plan), ["2", "3", "4"]);
    }

    /// The worked example from the ADR: #100 Payments, nested containers,
    /// a closed container over an open child, and an external blocker.
    /// Every rule does visible work to make #103 the only ready issue.
    fn payments() -> Plan {
        plan(
            100,
            vec![
                node(100, false, &[101, 102, 105, 107], &[]),
                node(101, true, &[], &[]),
                node(102, false, &[103, 104], &[]),
                node(103, false, &[], &[(101, true), (55, true)]),
                node(104, false, &[], &[(103, false)]),
                node(105, true, &[106], &[]),
                node(106, false, &[], &[]),
                node(107, false, &[], &[(102, false)]),
            ],
        )
    }

    #[test]
    fn nested_containers_schedule_their_leaves_through_the_one_predicate() {
        let plan = payments();
        assert_eq!(
            ready_ids(&plan),
            ["103"],
            "closed blockers release it; #104 waits, #107 waits on a container, #106 is abandoned"
        );
        assert!(plan.problems().is_empty());
    }

    #[test]
    fn a_blocker_on_a_container_holds_until_every_leaf_beneath_it_settles() {
        let plan = payments();
        let none = BTreeSet::new();
        assert!(
            !plan.settled(&id(102), &none),
            "an open container with open children is not settled"
        );
        let done: BTreeSet<IssueId> = [id(103), id(104)].into();
        assert!(
            plan.settled(&id(102), &done),
            "a container's doneness is its children's, never asserted"
        );
        let ready = plan.ready(&done);
        assert_eq!(
            ready
                .iter()
                .map(|issue| issue.id.0.as_str())
                .collect::<Vec<_>>(),
            ["107"],
            "the docs waited on the whole API"
        );
    }

    #[test]
    fn a_closed_container_stops_its_open_children() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2], &[]),
                node(2, true, &[3], &[]),
                node(3, false, &[], &[]),
            ],
        );
        assert!(
            ready_ids(&plan).is_empty(),
            "the subtree goes with its container"
        );
        assert_eq!(
            plan.problems(),
            [Problem::NoWork],
            "with its one subtree abandoned, the plan holds no work"
        );
    }

    #[test]
    fn an_external_blocker_holds_by_its_own_state() {
        let nodes = |outside_closed: bool| {
            vec![
                node(1, false, &[2], &[]),
                node(2, false, &[], &[(55, outside_closed)]),
            ]
        };
        let held = plan(1, nodes(false));
        assert_eq!(
            held.outside,
            [issue(55, false)],
            "recorded from the edge itself"
        );
        assert!(ready_ids(&held).is_empty());
        let released = plan(1, nodes(true));
        assert_eq!(ready_ids(&released), ["2"]);
        assert!(
            released.problems().is_empty(),
            "an outside edge is not dangling"
        );
    }

    #[test]
    fn a_cycle_is_named_and_nothing_on_it_is_ready() {
        let plan = two_and_three_in_a_cycle();
        assert!(ready_ids(&plan).is_empty());
        assert_eq!(
            plan.problems(),
            [Problem::Cycle(vec![id(2), id(3), id(2)])],
            "one cycle, found once"
        );
        assert_eq!(
            plan.problems()[0].to_string(),
            "blocked-by edges form a cycle: 2 -> 3 -> 2"
        );
    }

    #[test]
    fn a_dangling_edge_is_named_rather_than_scheduled_around() {
        let plan = Plan {
            tree: Tree {
                value: issue(1, false),
                children: vec![Tree::new(issue(2, false))],
            },
            blocking: vec![Blocking {
                issue: id(2),
                blocker: id(9),
            }],
            outside: Vec::new(),
        };
        assert_eq!(
            plan.problems(),
            [Problem::Dangling(Blocking {
                issue: id(2),
                blocker: id(9),
            })]
        );
        assert!(
            ready_ids(&plan).is_empty(),
            "an unknowable blocker never settles"
        );
    }

    #[test]
    fn a_plan_of_closed_issues_has_no_work_in_it() {
        let plan = plan(1, vec![node(1, false, &[2], &[]), node(2, true, &[], &[])]);
        assert_eq!(plan.problems(), [Problem::NoWork]);
        assert!(ready_ids(&plan).is_empty());
    }

    #[test]
    fn the_done_set_settles_an_issue_the_tracker_still_calls_open() {
        let plan = two_then_three();
        let done: BTreeSet<IssueId> = [id(2)].into();
        assert!(plan.settled(&id(2), &done));
        assert_eq!(
            plan.ready(&done)
                .iter()
                .map(|issue| issue.id.0.as_str())
                .collect::<Vec<_>>(),
            ["3"]
        );
    }

    #[test]
    fn an_exhausted_cap_is_a_refusal_in_words() {
        // An endless chain of containers: node n holds node n + 1.
        let error = Plan::descend(&id(0), |asked| {
            let number: u64 = asked.0.parse().unwrap();
            Ok(node(number, false, &[number + 1], &[]))
        })
        .unwrap_err();
        assert!(error.contains("truncated"), "{error}");
        assert!(error.contains("500"), "{error}");
    }

    #[test]
    fn a_containment_loop_terminates_and_keeps_the_first_place() {
        let plan = plan(
            1,
            vec![node(1, false, &[2], &[]), node(2, false, &[1], &[])],
        );
        let ids: Vec<&str> = plan.tree.nodes().map(|issue| issue.id.0.as_str()).collect();
        assert_eq!(ids, ["1", "2"], "the revisit is dropped, not recursed");
        assert_eq!(ready_ids(&plan), ["2"]);
    }

    #[test]
    fn a_fetch_failure_passes_through_in_its_own_words() {
        let error =
            Plan::descend(&id(1), |asked| Err(format!("no such issue: {asked}"))).unwrap_err();
        assert_eq!(error, "no such issue: 1");
    }

    #[test]
    fn problems_serialize_as_their_words() {
        assert_eq!(
            serde_json::to_value(Problem::NoWork).unwrap(),
            serde_json::json!("every issue is already settled: the plan has no work in it")
        );
    }

    #[test]
    fn the_work_is_the_open_leaves_under_no_settled_ancestor() {
        let plan = payments();
        let work: Vec<&str> = plan
            .work(&BTreeSet::new())
            .iter()
            .map(|issue| issue.id.0.as_str())
            .collect();
        assert_eq!(
            work,
            ["103", "104", "107"],
            "no containers, no closed leaves, no abandoned subtrees"
        );
    }

    fn doomed_pairs(plan: &Plan, lost: &[u64]) -> Vec<(String, String)> {
        let lost: BTreeSet<IssueId> = lost.iter().copied().map(id).collect();
        plan.doomed(&BTreeSet::new(), &lost)
            .into_iter()
            .map(|(leaf, blocker)| (leaf.0, blocker.0))
            .collect()
    }

    #[test]
    fn a_lost_leaf_dooms_its_dependents_transitively_and_spares_the_rest() {
        let plan = a_chain_beside_a_loner();
        assert!(doomed_pairs(&plan, &[]).is_empty(), "a healthy chain lives");
        assert_eq!(
            doomed_pairs(&plan, &[2]),
            [
                ("3".to_owned(), "2".to_owned()),
                ("5".to_owned(), "3".to_owned())
            ],
            "each stuck on the first blocker it waits for; 4 waited on no one"
        );
    }

    #[test]
    fn a_lost_leaf_under_a_container_dooms_whoever_waits_on_the_container() {
        let plan = nine_waiting_on_container_seven();
        assert_eq!(
            doomed_pairs(&plan, &[2]),
            [("9".to_owned(), "7".to_owned())],
            "7 can never settle; 8 is still work"
        );
    }

    #[test]
    fn an_open_outside_blocker_dooms_within_one_build() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[(55, false)]),
                node(3, false, &[], &[(2, false)]),
            ],
        );
        assert_eq!(
            doomed_pairs(&plan, &[]),
            [
                ("2".to_owned(), "55".to_owned()),
                ("3".to_owned(), "2".to_owned())
            ],
            "nothing in a build closes an outside issue"
        );
    }

    #[test]
    fn a_cycle_dooms_both_sides_rather_than_propping_them_up() {
        let plan = two_and_three_in_a_cycle();
        assert_eq!(
            doomed_pairs(&plan, &[]),
            [
                ("2".to_owned(), "3".to_owned()),
                ("3".to_owned(), "2".to_owned())
            ]
        );
    }

    #[test]
    fn a_dangling_blocker_dooms_the_leaf_that_waits_on_it() {
        let plan = Plan {
            tree: Tree {
                value: issue(1, false),
                children: vec![Tree::new(issue(2, false))],
            },
            blocking: vec![Blocking {
                issue: id(2),
                blocker: id(9),
            }],
            outside: Vec::new(),
        };
        assert_eq!(doomed_pairs(&plan, &[]), [("2".to_owned(), "9".to_owned())]);
    }

    #[test]
    fn a_blocker_that_is_abandoned_work_dooms_its_dependents() {
        // 6 is open but its container 5 is closed: 6 is not work, and it
        // will never close, so 3 can never become ready.
        let plan = plan(
            1,
            vec![
                node(1, false, &[3, 5], &[]),
                node(3, false, &[], &[(6, false)]),
                node(5, true, &[6], &[]),
                node(6, false, &[], &[]),
            ],
        );
        assert_eq!(doomed_pairs(&plan, &[]), [("3".to_owned(), "6".to_owned())]);
    }

    #[test]
    fn the_done_set_keeps_a_blocker_settleable() {
        let plan = two_then_three();
        let done: BTreeSet<IssueId> = [id(2)].into();
        assert!(plan.doomed(&done, &BTreeSet::new()).is_empty());
    }
}
