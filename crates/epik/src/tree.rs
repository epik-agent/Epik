//! A tree, and the one way to move around one.
//!
//! [`Tree<T>`] carries containment and nothing else: a value and the
//! subtrees beneath it. Reading it happens through three verbs —
//! [`nodes`](Tree::nodes), [`leaves`](Tree::leaves), and
//! [`find_path`](Tree::find_path) — and `find_path` is the only
//! navigation primitive: its one result yields the matched node, the
//! subtree it roots, its descendants, and its ancestors, so nothing
//! outside this module ever walks `children` by hand.

use serde::{Deserialize, Serialize};

/// A value and the subtrees beneath it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Tree<T> {
    pub value: T,
    pub children: Vec<Self>,
}

impl<T> Tree<T> {
    /// A tree of one node.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            value,
            children: Vec::new(),
        }
    }

    /// Every value in the tree, this node first, depth-first.
    pub fn nodes(&self) -> impl Iterator<Item = &T> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let tree = stack.pop()?;
            stack.extend(tree.children.iter().rev());
            Some(&tree.value)
        })
    }

    /// The values at the childless nodes, in [`nodes`](Self::nodes) order.
    /// Where the tree is a plan, these are the work.
    pub fn leaves(&self) -> impl Iterator<Item = &T> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            loop {
                let tree = stack.pop()?;
                stack.extend(tree.children.iter().rev());
                if tree.children.is_empty() {
                    return Some(&tree.value);
                }
            }
        })
    }

    /// The chain of subtrees from this root down to the first node — in
    /// [`nodes`](Self::nodes) order — whose value satisfies `found`, or
    /// `None` when nothing does. The last element is the match's own
    /// subtree, everything before it is an ancestor, and the last
    /// element's `nodes` are the match and its descendants: one result,
    /// every direction.
    pub fn find_path(&self, found: impl Fn(&T) -> bool) -> Option<Vec<&Self>> {
        fn descend<'a, T>(
            tree: &'a Tree<T>,
            found: &impl Fn(&T) -> bool,
            path: &mut Vec<&'a Tree<T>>,
        ) -> bool {
            path.push(tree);
            if found(&tree.value)
                || tree
                    .children
                    .iter()
                    .any(|child| descend(child, found, path))
            {
                return true;
            }
            path.pop();
            false
        }
        let mut path = Vec::new();
        descend(self, &found, &mut path).then_some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 holding (2 holding 4 and 5) and 3.
    fn tree() -> Tree<u64> {
        Tree {
            value: 1,
            children: vec![
                Tree {
                    value: 2,
                    children: vec![Tree::new(4), Tree::new(5)],
                },
                Tree::new(3),
            ],
        }
    }

    #[test]
    fn nodes_visits_every_value_root_first() {
        let visited: Vec<u64> = tree().nodes().copied().collect();
        assert_eq!(visited, [1, 2, 4, 5, 3]);
    }

    #[test]
    fn leaves_are_the_childless_nodes_in_the_same_order() {
        let leaves: Vec<u64> = tree().leaves().copied().collect();
        assert_eq!(leaves, [4, 5, 3]);
    }

    #[test]
    fn a_lone_node_is_its_own_leaf() {
        let lone = Tree::new(7);
        assert_eq!(lone.leaves().copied().collect::<Vec<_>>(), [7]);
        assert_eq!(lone.nodes().copied().collect::<Vec<_>>(), [7]);
    }

    #[test]
    fn find_path_is_the_chain_from_the_root_to_the_match() {
        let tree = tree();
        let path = tree.find_path(|value| *value == 5).unwrap();
        let values: Vec<u64> = path.iter().map(|tree| tree.value).collect();
        assert_eq!(values, [1, 2, 5], "the root, the ancestors, the match");
    }

    #[test]
    fn find_paths_last_element_carries_the_match_and_its_descendants() {
        let tree = tree();
        let path = tree.find_path(|value| *value == 2).unwrap();
        let subtree = path.last().unwrap();
        assert_eq!(subtree.nodes().copied().collect::<Vec<_>>(), [2, 4, 5]);
    }

    #[test]
    fn find_path_answers_none_when_nothing_matches() {
        assert_eq!(tree().find_path(|value| *value == 9), None);
    }

    #[test]
    fn find_path_takes_the_first_match_in_nodes_order() {
        let tree = tree();
        let path = tree.find_path(|value| *value > 3).unwrap();
        assert_eq!(path.last().unwrap().value, 4);
    }
}
