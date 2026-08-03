use std::collections::VecDeque;

#[derive(Debug)]
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
