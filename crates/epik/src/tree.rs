use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Tree<T> {
    pub value: T,
    pub children: Vec<Self>,
}

impl<T> Tree<T> {
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            value,
            children: Vec::new(),
        }
    }

    /// Maps every value in the tree, preserving its shape.
    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Tree<U> {
        fn go<T, U>(tree: Tree<T>, f: &mut impl FnMut(T) -> U) -> Tree<U> {
            Tree {
                value: f(tree.value),
                children: tree
                    .children
                    .into_iter()
                    .map(|child| go(child, &mut *f))
                    .collect(),
            }
        }
        go(self, &mut f)
    }

    pub fn bfs<'a>(&'a self, mut visit: impl FnMut(&'a T)) {
        let mut queue = VecDeque::from([self]);
        while let Some(node) = queue.pop_front() {
            visit(&node.value);
            queue.extend(node.children.iter());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bfs_visits_breadth_first() {
        let tree = Tree {
            value: 1,
            children: vec![
                Tree {
                    value: 2,
                    children: vec![Tree::new(4), Tree::new(5)],
                },
                Tree::new(3),
            ],
        };

        let mut visited = Vec::new();
        tree.bfs(|v| visited.push(*v));
        assert_eq!(visited, [1, 2, 3, 4, 5]);
    }
}
