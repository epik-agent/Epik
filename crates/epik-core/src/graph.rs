//! The dependency graph of an epic's issues: what can run now, and what
//! never can.
//!
//! A [`Graph`] is built from each issue's blockers. It is pure and
//! synchronous: it reads nothing from GitHub, and the caller says which
//! issues are closed, running, or failed.

use std::collections::{BTreeMap, BTreeSet};

/// An issue's number.
pub type Issue = u64;

/// An epic's issues and the issues blocking each.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Graph {
    blockers: BTreeMap<Issue, BTreeSet<Issue>>,
}

impl Graph {
    /// The graph of `blockers`, each issue with the issues blocking
    /// it. Blockers outside the graph are ignored, as `epic_status`
    /// ignores them.
    pub fn new<B>(blockers: impl IntoIterator<Item = (Issue, B)>) -> Self
    where
        B: IntoIterator<Item = Issue>,
    {
        let given: BTreeMap<Issue, BTreeSet<Issue>> = blockers
            .into_iter()
            .map(|(issue, blockers)| (issue, blockers.into_iter().collect()))
            .collect();
        let blockers = given
            .iter()
            .map(|(&issue, blockers)| {
                let inside = blockers.iter().copied().filter(|b| given.contains_key(b));
                (issue, inside.collect())
            })
            .collect();
        Self { blockers }
    }

    /// Each cycle's issues, sorted, the cycles in order of their least
    /// issue. An issue blocking itself is a cycle.
    pub fn cycles(&self) -> Vec<Vec<Issue>> {
        Tarjan::run(self).cycles
    }

    /// Each issue's level: 0 with no blockers, else one more than its
    /// deepest blocker's. `None` for an issue on a cycle or behind one.
    pub fn levels(&self) -> BTreeMap<Issue, Option<usize>> {
        Tarjan::run(self).levels
    }

    /// The open issues that can start now, in number order: neither
    /// running nor failed, with every blocker closed.
    pub fn ready(
        &self,
        closed: &BTreeSet<Issue>,
        running: &BTreeSet<Issue>,
        failed: &BTreeSet<Issue>,
    ) -> Vec<Issue> {
        self.blockers
            .iter()
            .filter(|(issue, _)| {
                !closed.contains(issue) && !running.contains(issue) && !failed.contains(issue)
            })
            .filter(|(_, blockers)| blockers.is_subset(closed))
            .map(|(&issue, _)| issue)
            .collect()
    }

    /// The open issues that can never run because a blocker failed,
    /// directly or through other open issues, in number order. A failed
    /// issue is not itself stuck.
    pub fn stuck(&self, closed: &BTreeSet<Issue>, failed: &BTreeSet<Issue>) -> Vec<Issue> {
        let mut stuck = BTreeSet::new();
        let mut reached: Vec<Issue> = failed.iter().copied().collect();
        while let Some(blocker) = reached.pop() {
            for (&issue, blockers) in &self.blockers {
                if blockers.contains(&blocker)
                    && !closed.contains(&issue)
                    && !failed.contains(&issue)
                    && stuck.insert(issue)
                {
                    reached.push(issue);
                }
            }
        }
        stuck.into_iter().collect()
    }
}

/// Tarjan's algorithm over a [`Graph`]. It emits each strongly connected component
/// after every component it can reach, so an issue's blockers are leveled
/// before it is.
struct Tarjan<'g> {
    graph: &'g Graph,
    index: BTreeMap<Issue, usize>,
    low: BTreeMap<Issue, usize>,
    stack: Vec<Issue>,
    on_stack: BTreeSet<Issue>,
    levels: BTreeMap<Issue, Option<usize>>,
    cycles: Vec<Vec<Issue>>,
}

impl<'g> Tarjan<'g> {
    fn run(graph: &'g Graph) -> Self {
        let mut tarjan = Self {
            graph,
            index: BTreeMap::new(),
            low: BTreeMap::new(),
            stack: Vec::new(),
            on_stack: BTreeSet::new(),
            levels: BTreeMap::new(),
            cycles: Vec::new(),
        };
        for &issue in graph.blockers.keys() {
            if !tarjan.index.contains_key(&issue) {
                tarjan.visit(issue);
            }
        }
        tarjan.cycles.sort();
        tarjan
    }

    fn visit(&mut self, issue: Issue) {
        let index = self.index.len();
        self.index.insert(issue, index);
        self.low.insert(issue, index);
        self.stack.push(issue);
        self.on_stack.insert(issue);
        let blockers = &self.graph.blockers[&issue];
        for &blocker in blockers {
            let reach = if !self.index.contains_key(&blocker) {
                self.visit(blocker);
                self.low[&blocker]
            } else if self.on_stack.contains(&blocker) {
                self.index[&blocker]
            } else {
                continue;
            };
            let low = self.low[&issue].min(reach);
            self.low.insert(issue, low);
        }
        if self.low[&issue] != self.index[&issue] {
            return;
        }
        let at = self.stack.iter().rposition(|&on| on == issue);
        let mut component = self
            .stack
            .split_off(at.expect("an issue being visited is on the stack"));
        for node in &component {
            self.on_stack.remove(node);
        }
        if component.len() > 1 || blockers.contains(&issue) {
            component.sort();
            for &node in &component {
                self.levels.insert(node, None);
            }
            self.cycles.push(component);
        } else {
            let below: Option<Vec<usize>> = blockers.iter().map(|b| self.levels[b]).collect();
            let level = below.map(|below| below.into_iter().max().map_or(0, |deepest| deepest + 1));
            self.levels.insert(issue, level);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph<const N: usize>(edges: [(Issue, &[Issue]); N]) -> Graph {
        Graph::new(edges.map(|(issue, blockers)| (issue, blockers.to_vec())))
    }

    fn set<const N: usize>(issues: [Issue; N]) -> BTreeSet<Issue> {
        BTreeSet::from(issues)
    }

    fn levels<const N: usize>(
        levels: [(Issue, Option<usize>); N],
    ) -> BTreeMap<Issue, Option<usize>> {
        BTreeMap::from(levels)
    }

    #[test]
    fn a_chain_levels_one_after_another() {
        let chain = graph([(1, &[]), (2, &[1]), (3, &[2])]);
        assert_eq!(chain.cycles(), Vec::<Vec<Issue>>::new());
        assert_eq!(
            chain.levels(),
            levels([(1, Some(0)), (2, Some(1)), (3, Some(2))])
        );
        assert_eq!(chain.ready(&set([]), &set([]), &set([])), [1]);
        assert_eq!(chain.ready(&set([1]), &set([]), &set([])), [2]);
        assert_eq!(chain.ready(&set([1, 2]), &set([]), &set([])), [3]);
        assert_eq!(
            chain.ready(&set([1, 2, 3]), &set([]), &set([])),
            Vec::<Issue>::new()
        );
    }

    #[test]
    fn a_diamond_levels_by_its_deepest_blocker() {
        let diamond = graph([(1, &[]), (2, &[1]), (3, &[1]), (4, &[2, 3]), (5, &[1, 4])]);
        assert_eq!(diamond.cycles(), Vec::<Vec<Issue>>::new());
        assert_eq!(
            diamond.levels(),
            levels([
                (1, Some(0)),
                (2, Some(1)),
                (3, Some(1)),
                (4, Some(2)),
                (5, Some(3))
            ])
        );
        assert_eq!(diamond.ready(&set([1]), &set([]), &set([])), [2, 3]);
        assert_eq!(diamond.ready(&set([1, 2]), &set([]), &set([])), [3]);
        assert_eq!(diamond.ready(&set([1, 2, 3]), &set([]), &set([])), [4]);
    }

    #[test]
    fn independent_issues_are_all_ready_at_level_zero() {
        let independent = graph([(7, &[]), (3, &[]), (5, &[])]);
        assert_eq!(independent.cycles(), Vec::<Vec<Issue>>::new());
        assert_eq!(
            independent.levels(),
            levels([(3, Some(0)), (5, Some(0)), (7, Some(0))])
        );
        assert_eq!(independent.ready(&set([]), &set([]), &set([])), [3, 5, 7]);
    }

    #[test]
    fn a_cycle_and_what_it_blocks_have_no_level() {
        let cyclic = graph([
            (1, &[]),
            (2, &[1, 4]),
            (3, &[2]),
            (4, &[3]),
            (5, &[4]),
            (6, &[1]),
        ]);
        assert_eq!(cyclic.cycles(), [vec![2, 3, 4]]);
        assert_eq!(
            cyclic.levels(),
            levels([
                (1, Some(0)),
                (2, None),
                (3, None),
                (4, None),
                (5, None),
                (6, Some(1))
            ])
        );
        assert_eq!(cyclic.ready(&set([1]), &set([]), &set([])), [6]);
    }

    #[test]
    fn cycles_are_ordered_by_their_least_issue() {
        let cyclic = graph([(9, &[8]), (8, &[9]), (5, &[2]), (2, &[5])]);
        assert_eq!(cyclic.cycles(), [vec![2, 5], vec![8, 9]]);
    }

    #[test]
    fn a_self_block_is_a_cycle() {
        let selfish = graph([(1, &[1]), (2, &[1]), (3, &[])]);
        assert_eq!(selfish.cycles(), [vec![1]]);
        assert_eq!(
            selfish.levels(),
            levels([(1, None), (2, None), (3, Some(0))])
        );
        assert_eq!(selfish.ready(&set([]), &set([]), &set([])), [3]);
    }

    #[test]
    fn a_blocker_outside_the_epic_is_ignored() {
        let outside = graph([(1, &[100]), (2, &[1, 200])]);
        assert_eq!(outside.cycles(), Vec::<Vec<Issue>>::new());
        assert_eq!(outside.levels(), levels([(1, Some(0)), (2, Some(1))]));
        assert_eq!(outside.ready(&set([]), &set([]), &set([])), [1]);
    }

    #[test]
    fn ready_skips_closed_running_and_failed_issues() {
        let independent = graph([(1, &[]), (2, &[]), (3, &[]), (4, &[])]);
        assert_eq!(independent.ready(&set([1]), &set([2]), &set([3])), [4]);
    }

    #[test]
    fn a_failed_blocker_keeps_its_dependents_from_ready() {
        let chain = graph([(1, &[]), (2, &[1]), (3, &[])]);
        assert_eq!(chain.ready(&set([]), &set([]), &set([1])), [3]);
    }

    #[test]
    fn stuck_follows_a_failure_through_open_issues() {
        let diamond = graph([
            (1, &[]),
            (2, &[1]),
            (3, &[1]),
            (4, &[2, 3]),
            (5, &[]),
            (6, &[5]),
        ]);
        assert_eq!(diamond.stuck(&set([1]), &set([2])), [4]);
        assert_eq!(diamond.stuck(&set([]), &set([1])), [2, 3, 4]);
        assert_eq!(diamond.stuck(&set([]), &set([5])), [6]);
        assert_eq!(diamond.stuck(&set([]), &set([])), Vec::<Issue>::new());
    }

    #[test]
    fn a_closed_issue_is_never_stuck() {
        let chain = graph([(1, &[]), (2, &[1]), (3, &[2])]);
        assert_eq!(chain.stuck(&set([2]), &set([1])), Vec::<Issue>::new());
    }

    #[test]
    fn a_failed_issue_is_not_itself_stuck() {
        let chain = graph([(1, &[]), (2, &[1])]);
        assert_eq!(chain.stuck(&set([]), &set([1, 2])), Vec::<Issue>::new());
    }
}
