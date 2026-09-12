//! A directed acyclic graph, and the few ways to move around one.
//!
//! [`Dag<T>`] carries order: values, and which of them come before
//! which. Every `Dag` is acyclic — the one door in, [`Dag::new`],
//! checks, and deserializing goes through it — so every reading below
//! is total. Reading it happens through two verbs: [`nodes`](Dag::nodes)
//! is every value with its predecessors before it, each a [`NodeRef`]
//! into the graph, and [`find`](Dag::find) is the first of those to
//! satisfy a predicate. A node reference knows what is directly before
//! it and everything before and after it: every walk of the graph is
//! here.

use std::collections::BTreeSet;
use std::fmt::Write;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize, Serializer};

/// A value and what comes before it, each by position in the [`Dag`].
/// Wire vocabulary: this is how a graph's shape crosses to a model or
/// a window and comes back.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Node<T> {
    pub value: T,
    /// Positions of the nodes directly before this one.
    pub predecessors: Vec<usize>,
}

impl<T> Node<T> {
    /// A node after `predecessors`, by position; none, for one that
    /// can come first.
    pub fn new(value: T, predecessors: impl IntoIterator<Item = usize>) -> Self {
        Self {
            value,
            predecessors: predecessors.into_iter().collect(),
        }
    }
}

/// Values and the order among them. Serializes as its [`Node`]s, in
/// the order given, and deserializes through [`Dag::new`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "Vec<Node<T>>")]
pub struct Dag<T> {
    nodes: Vec<Node<T>>,
    /// Positions with every predecessor before the node it precedes,
    /// and otherwise as given. Settled once, at construction.
    order: Vec<usize>,
}

impl<T> Dag<T> {
    /// `nodes` as a graph.
    ///
    /// # Errors
    ///
    /// A predecessor names a position past the last node, or the
    /// predecessors run in a cycle — a node preceding itself included.
    /// The words name the positions.
    pub fn new(nodes: Vec<Node<T>>) -> Result<Self> {
        let order = topological_order(&nodes)?;
        Ok(Self { nodes, order })
    }

    /// Every value, each after everything before it and otherwise in
    /// the order given, each by reference into the graph. `&dag` in a
    /// `for` loop is the same walk.
    pub fn nodes(&self) -> Nodes<'_, T> {
        Nodes {
            dag: self,
            order: self.order.iter(),
        }
    }

    /// The first value — in [`nodes`](Self::nodes) order — satisfying
    /// `found`, by reference into the graph; `None` when every value
    /// fails it.
    pub fn find(&self, found: impl Fn(&T) -> bool) -> Option<NodeRef<'_, T>> {
        self.nodes().find(|node| found(node.value()))
    }

    /// The first value — in [`nodes`](Self::nodes) order — satisfying
    /// `found`, to change, the shape around it kept.
    pub fn find_mut(&mut self, found: impl Fn(&T) -> bool) -> Option<&mut T> {
        let at = self
            .order
            .iter()
            .copied()
            .find(|&at| found(&self.nodes[at].value))?;
        Some(&mut self.nodes[at].value)
    }

    /// The value at `position` — as [`NodeRef::position`] gave it — to
    /// change, the shape around it kept; `None` past the last node.
    pub fn get_mut(&mut self, position: usize) -> Option<&mut T> {
        self.nodes.get_mut(position).map(|node| &mut node.value)
    }

    /// The same shape over `f` of each value.
    pub fn map<U>(self, f: impl Fn(T) -> U) -> Dag<U> {
        Dag {
            nodes: self
                .nodes
                .into_iter()
                .map(|node| Node {
                    value: f(node.value),
                    predecessors: node.predecessors,
                })
                .collect(),
            order: self.order,
        }
    }

    /// Every position reachable from `at` by following `step`, which
    /// names each position's neighbours: the neighbours' closure.
    fn reach(&self, at: usize, step: impl Fn(usize) -> Vec<usize>) -> BTreeSet<usize> {
        let mut reached = BTreeSet::new();
        let mut stack = step(at);
        while let Some(next) = stack.pop() {
            if reached.insert(next) {
                stack.extend(step(next));
            }
        }
        reached
    }

    /// The values at `positions`, by reference, in
    /// [`nodes`](Self::nodes) order.
    fn in_order(&self, positions: BTreeSet<usize>) -> impl Iterator<Item = NodeRef<'_, T>> {
        self.order
            .iter()
            .filter(move |at| positions.contains(at))
            .map(|&at| NodeRef { dag: self, at })
    }

    /// The positions directly after `at`.
    fn successors(&self, at: usize) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|&other| self.nodes[other].predecessors.contains(&at))
            .collect()
    }
}

/// A value where it sits in a [`Dag`]: what is directly before it,
/// and everything before and after it.
#[derive(Debug)]
pub struct NodeRef<'a, T> {
    dag: &'a Dag<T>,
    at: usize,
}

// By hand: a reference and a position are copyable whatever `T` is.
impl<T> Clone for NodeRef<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for NodeRef<'_, T> {}

impl<'a, T> NodeRef<'a, T> {
    /// The value itself.
    #[must_use]
    pub fn value(self) -> &'a T {
        &self.dag.nodes[self.at].value
    }

    /// Where the value sits: its position among the nodes as given,
    /// which [`Dag::get_mut`] takes.
    #[must_use]
    pub const fn position(self) -> usize {
        self.at
    }

    /// The values directly before this one, by reference, in
    /// [`nodes`](Dag::nodes) order.
    pub fn predecessors(self) -> impl Iterator<Item = NodeRef<'a, T>> {
        let direct = self.dag.nodes[self.at]
            .predecessors
            .iter()
            .copied()
            .collect();
        self.dag.in_order(direct)
    }

    /// Everything before this value, directly or through another,
    /// by reference, in [`nodes`](Dag::nodes) order.
    pub fn ancestors(self) -> impl Iterator<Item = NodeRef<'a, T>> {
        let dag = self.dag;
        let before = dag.reach(self.at, |at| dag.nodes[at].predecessors.clone());
        dag.in_order(before)
    }

    /// Everything after this value, directly or through another,
    /// by reference, in [`nodes`](Dag::nodes) order.
    pub fn descendants(self) -> impl Iterator<Item = NodeRef<'a, T>> {
        let dag = self.dag;
        let after = dag.reach(self.at, |at| dag.successors(at));
        dag.in_order(after)
    }
}

/// The walk [`Dag::nodes`] gives: every value, each after everything
/// before it.
#[derive(Debug)]
pub struct Nodes<'a, T> {
    dag: &'a Dag<T>,
    order: std::slice::Iter<'a, usize>,
}

impl<'a, T> Iterator for Nodes<'a, T> {
    type Item = NodeRef<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        let dag = self.dag;
        self.order.next().map(|&at| NodeRef { dag, at })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.order.size_hint()
    }
}

impl<T> DoubleEndedIterator for Nodes<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let dag = self.dag;
        self.order.next_back().map(|&at| NodeRef { dag, at })
    }
}

impl<T> ExactSizeIterator for Nodes<'_, T> {}

impl<'a, T> IntoIterator for &'a Dag<T> {
    type Item = NodeRef<'a, T>;
    type IntoIter = Nodes<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.nodes()
    }
}

impl<T> TryFrom<Vec<Node<T>>> for Dag<T> {
    type Error = anyhow::Error;

    fn try_from(nodes: Vec<Node<T>>) -> Result<Self> {
        Self::new(nodes)
    }
}

impl<T: Serialize> Serialize for Dag<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.nodes.serialize(serializer)
    }
}

/// Positions with every predecessor before the node it precedes, and
/// otherwise as given — or the cycle that makes that impossible, or
/// the predecessor past the last node.
fn topological_order<T>(nodes: &[Node<T>]) -> Result<Vec<usize>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unseen,
        Visiting,
        Done,
    }
    fn visit<T>(
        at: usize,
        nodes: &[Node<T>],
        marks: &mut [Mark],
        trail: &mut Vec<usize>,
        order: &mut Vec<usize>,
    ) -> Result<()> {
        match marks[at] {
            Mark::Done => return Ok(()),
            Mark::Visiting => {
                let start = trail
                    .iter()
                    .position(|&seen| seen == at)
                    .unwrap_or_default();
                let mut cycle = String::new();
                for seen in &trail[start..] {
                    write!(cycle, "{seen} after ")?;
                }
                bail!("the predecessors run in a cycle: {cycle}{at}");
            }
            Mark::Unseen => {}
        }
        marks[at] = Mark::Visiting;
        trail.push(at);
        for &predecessor in &nodes[at].predecessors {
            if predecessor >= nodes.len() {
                bail!(
                    "{at} comes after {predecessor}, and there are {} nodes",
                    nodes.len()
                );
            }
            visit(predecessor, nodes, marks, trail, order)?;
        }
        trail.pop();
        marks[at] = Mark::Done;
        order.push(at);
        Ok(())
    }
    let mut marks = vec![Mark::Unseen; nodes.len()];
    let mut order = Vec::with_capacity(nodes.len());
    for at in 0..nodes.len() {
        visit(at, nodes, &mut marks, &mut Vec::new(), &mut order)?;
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A diamond and a loner: 4 after 2 and 3, which each come after
    /// 1; 5 first. Given out of order, so `nodes` has work to do.
    fn dag() -> Dag<u64> {
        Dag::new(vec![
            Node::new(4, [1, 2]),
            Node::new(2, [3]),
            Node::new(3, [3]),
            Node::new(1, []),
            Node::new(5, []),
        ])
        .unwrap()
    }

    fn values<'a>(nodes: impl Iterator<Item = NodeRef<'a, u64>>) -> Vec<u64> {
        nodes.map(|node| *node.value()).collect()
    }

    #[test]
    fn nodes_puts_every_predecessor_before_the_node_it_precedes() {
        assert_eq!(values(dag().nodes()), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_for_loop_over_the_dag_is_the_nodes_walk() {
        let dag = dag();
        let mut walked = Vec::new();
        for node in &dag {
            walked.push(*node.value());
        }
        assert_eq!(walked, [1, 2, 3, 4, 5]);
        assert_eq!(dag.nodes().len(), 5);
        assert_eq!(dag.nodes().next_back().map(NodeRef::value), Some(&5));
    }

    #[test]
    fn a_lone_node_stands_alone() {
        let lone = Dag::new(vec![Node::new(7, [])]).unwrap();
        let node = lone.find(|value| *value == 7).unwrap();
        assert_eq!(*node.value(), 7);
        assert!(values(node.predecessors()).is_empty());
        assert!(values(node.ancestors()).is_empty());
        assert!(values(node.descendants()).is_empty());
    }

    #[test]
    fn a_node_ref_knows_every_direction_from_it() {
        let dag = dag();
        let node = dag.find(|value| *value == 4).unwrap();
        assert_eq!(values(node.predecessors()), [2, 3]);
        assert_eq!(values(node.ancestors()), [1, 2, 3], "through 2 and 3 to 1");
        assert!(values(node.descendants()).is_empty());

        let node = dag.find(|value| *value == 1).unwrap();
        assert!(values(node.predecessors()).is_empty());
        assert_eq!(values(node.descendants()), [2, 3, 4], "the diamond alone");
    }

    #[test]
    fn find_answers_none_when_every_value_fails_the_predicate() {
        assert!(dag().find(|value| *value == 9).is_none());
    }

    #[test]
    fn find_takes_the_first_match_in_nodes_order() {
        let dag = dag();
        assert_eq!(*dag.find(|value| *value > 2).unwrap().value(), 3);
    }

    #[test]
    fn find_mut_changes_the_value_and_keeps_the_shape() {
        let mut dag = dag();
        *dag.find_mut(|value| *value == 2).unwrap() = 20;
        assert_eq!(values(dag.nodes()), [1, 20, 3, 4, 5]);
        let node = dag.find(|value| *value == 4).unwrap();
        assert_eq!(values(node.predecessors()), [20, 3]);
    }

    #[test]
    fn a_position_finds_the_value_again_to_change() {
        let mut dag = dag();
        let at = dag.find(|value| *value == 2).unwrap().position();
        assert_eq!(at, 1, "as given, the second node");
        *dag.get_mut(at).unwrap() = 20;
        assert_eq!(*dag.find(|value| *value == 20).unwrap().value(), 20);
        assert!(dag.get_mut(5).is_none());
    }

    #[test]
    fn map_keeps_the_shape() {
        let doubled = dag().map(|value| value * 2);
        assert_eq!(values(doubled.nodes()), [2, 4, 6, 8, 10]);
        let node = doubled.find(|value| *value == 8).unwrap();
        assert_eq!(values(node.ancestors()), [2, 4, 6]);
    }

    #[test]
    fn a_cycle_is_refused_in_words_that_name_it() {
        let error = Dag::new(vec![
            Node::new('a', [1]),
            Node::new('b', [2]),
            Node::new('c', [1]),
        ])
        .expect_err("a cycle")
        .to_string();
        assert_eq!(error, "the predecessors run in a cycle: 1 after 2 after 1");

        let error = Dag::new(vec![Node::new('a', [0])]).unwrap_err().to_string();
        assert_eq!(error, "the predecessors run in a cycle: 0 after 0");
    }

    #[test]
    fn a_predecessor_with_no_node_is_refused() {
        let error = Dag::new(vec![Node::new('a', [3])]).unwrap_err().to_string();
        assert_eq!(error, "0 comes after 3, and there are 1 nodes");
    }

    #[test]
    fn a_dag_crosses_the_wire_as_its_nodes_and_comes_back_itself() {
        let dag = dag();
        let wire = serde_json::to_string(&dag).unwrap();
        assert!(
            wire.starts_with(r#"[{"value":4,"predecessors":[1,2]},"#),
            "{wire}"
        );
        assert_eq!(serde_json::from_str::<Dag<u64>>(&wire).unwrap(), dag);
    }

    #[test]
    fn a_cycle_on_the_wire_is_refused_at_the_door() {
        let wire = r#"[{"value":1,"predecessors":[1]},{"value":2,"predecessors":[0]}]"#;
        let error = serde_json::from_str::<Dag<u64>>(wire)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cycle"), "{error}");
    }
}
