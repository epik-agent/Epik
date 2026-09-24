//! The work as a picture: x from the order, y from the graph's own.
//!
//! [`layout`] is the pure function from the work a build runs and the
//! states derived for it to a picture in grid cells, so a pane has
//! only to scale it. A node's row is its place in the graph's order,
//! which a [`Dag`] settles topologically; its column is its longest
//! path over the graph's edges, which one forward pass over those
//! rows settles, every predecessor being an earlier row. The states
//! decide what each node reads as: [`Form`] is the state with Waiting
//! split into ready and blocked, ready being every predecessor
//! merged. A node carries its attempts beside its state, an issue
//! going round its loop more than once being something a pane says.
//!
//! A [`Container`] is a box around a sub-feature's nodes. The work a
//! build runs is a flat [`Dag`] of issue numbers, which carries no
//! containment, so nothing draws one yet; the vocabulary stands for
//! the shape that will.

use std::collections::{BTreeMap, BTreeSet};

use crate::dag::Dag;

use super::State;

/// How a node is drawn: its state, with Waiting split by what has
/// merged into ready and blocked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Form {
    /// Waiting on a blocker.
    Blocked,
    /// Waiting on a slot: every blocker has merged.
    Ready,
    Running,
    Merging,
    Merged,
    Failed,
    Skipped,
}

/// One issue of work, placed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub id: u64,
    pub title: String,
    pub state: State,
    pub form: Form,
    /// The times an agent has run on it: one attempt each, counting
    /// the first, so a second says the issue went back for a fix.
    pub attempts: usize,
    /// The longest path over the blocking edges into this node.
    pub column: usize,
    /// Its place in the graph's order, among the drawn.
    pub row: usize,
    /// The blockers still holding it: the predecessors not yet merged.
    pub blocked_by: Vec<u64>,
    /// The nodes its merging helps release: its direct successors.
    pub blocks: Vec<u64>,
}

/// One arrow: `blocker` merges before `issue` starts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edge {
    pub blocker: u64,
    pub issue: u64,
}

/// One box: a sub-feature, with the drawn nodes in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Container {
    pub id: u64,
    pub title: String,
    /// How far beneath the feature: its child is 1.
    pub depth: usize,
    /// How many boxes nest inside it, at the deepest.
    pub height: usize,
    /// The nodes inside it, in row order.
    pub leaves: Vec<u64>,
}

/// The picture, in grid cells.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Layout {
    /// In row order.
    pub nodes: Vec<Node>,
    /// In (issue row, blocker row) order, each once.
    pub edges: Vec<Edge>,
    /// In tree order, so an outer box comes before the boxes in it.
    /// Empty while the work a build runs carries no containment.
    pub containers: Vec<Container>,
}

impl Layout {
    /// How many columns the picture spans.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.column + 1)
            .max()
            .unwrap_or(0)
    }

    /// How many rows: one per node.
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn node(&self, id: u64) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

/// The picture of `work` with every issue drawn where `states` has it
/// standing, titled as `titles` names it and carrying the `attempts`
/// its agent has made. Pure: the same arguments give the same picture.
#[must_use]
pub fn layout(
    work: &Dag<u64>,
    titles: &BTreeMap<u64, String>,
    states: &BTreeMap<u64, State>,
    attempts: &BTreeMap<u64, usize>,
) -> Layout {
    // The rows: every issue of the work, in the graph's own order.
    let drawn: Vec<u64> = work.nodes().map(|node| *node.value()).collect();
    let row_of: BTreeMap<u64, usize> = drawn
        .iter()
        .enumerate()
        .map(|(row, &id)| (id, row))
        .collect();

    // The edges as (issue row, blocker row).
    let edges: BTreeSet<(usize, usize)> = work
        .nodes()
        .flat_map(|node| {
            let issue = row_of[node.value()];
            let blockers: Vec<usize> = node
                .predecessors()
                .map(|before| row_of[before.value()])
                .collect();
            blockers.into_iter().map(move |blocker| (issue, blocker))
        })
        .collect();
    let mut into = vec![Vec::new(); drawn.len()];
    let mut blocks = vec![Vec::new(); drawn.len()];
    for &(issue, blocker) in &edges {
        into[issue].push(blocker);
        blocks[blocker].push(drawn[issue]);
    }
    // Every predecessor is an earlier row, the graph's order being
    // topological, so one forward pass settles every column.
    let mut column = vec![0; drawn.len()];
    for row in 0..drawn.len() {
        column[row] = into[row]
            .iter()
            .map(|&blocker| column[blocker] + 1)
            .max()
            .unwrap_or(0);
    }

    let merged = |id: &u64| matches!(states.get(id), Some(State::Merged { .. }));
    let nodes = drawn
        .iter()
        .zip(blocks)
        .enumerate()
        .map(|(row, (&id, blocks))| {
            let state = states.get(&id).cloned().unwrap_or(State::Waiting);
            let blocked_by: Vec<u64> = into[row]
                .iter()
                .map(|&blocker| drawn[blocker])
                .filter(|blocker| !merged(blocker))
                .collect();
            Node {
                id,
                title: titles.get(&id).cloned().unwrap_or_default(),
                form: form(&state, blocked_by.is_empty()),
                state,
                attempts: attempts.get(&id).copied().unwrap_or(0),
                column: column[row],
                row,
                blocked_by,
                blocks,
            }
        })
        .collect();

    Layout {
        nodes,
        edges: edges
            .into_iter()
            .map(|(issue, blocker)| Edge {
                blocker: drawn[blocker],
                issue: drawn[issue],
            })
            .collect(),
        containers: Vec::new(),
    }
}

const fn form(state: &State, ready: bool) -> Form {
    match state {
        State::Waiting if ready => Form::Ready,
        State::Waiting => Form::Blocked,
        State::Running => Form::Running,
        State::Merging => Form::Merging,
        State::Merged { .. } => Form::Merged,
        State::Failed { .. } => Form::Failed,
        State::Skipped { .. } => Form::Skipped,
    }
}
