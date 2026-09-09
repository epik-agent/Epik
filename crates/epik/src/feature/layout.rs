//! The plan as a picture: x from the DAG, y from the tree.
//!
//! A plan is two structures over one set of nodes, and a pane draws each
//! as itself — containment as boxes around their descendants, ordering
//! as arrows between nodes. [`layout`] is the pure function from a plan
//! and its states to that picture, in grid cells, so a pane has only to
//! scale it: a leaf's column is its longest path over the blocking
//! edges, its row is its place in a depth-first walk of the tree with
//! the empty rows squeezed out. The walk is what keeps a container's
//! leaves contiguous, which is what makes its box drawable without
//! swallowing a node that is not in it.
//!
//! The states decide what is drawn at all. Only the work — the leaves
//! the build holds a state for — is a node; a settled leaf or an
//! abandoned subtree is not, and neither is the feature itself, whose
//! box is the pane. Edges follow the plan's own reading of them: an edge
//! onto a container is an edge onto every drawn leaf beneath it, which
//! is how [`Plan::settled`] reads a container; an edge whose blocker is
//! outside the tree, or unknown to it, has no node to start from, holds
//! the leaf just the same, and is named in the leaf's `blocked_by`; an
//! edge whose *issue* side is a container is one the schedule never
//! consults — [`Plan::ready`] asks only for a leaf's own edges — and is
//! not drawn. A blocked-by cycle contributes no column past the edge
//! that closes it, so the layout ends on any plan.

use std::collections::{BTreeMap, BTreeSet};

use super::{Issue, IssueId, Plan, State};

/// How a node is drawn: its state, with Waiting split by the plan's
/// own judgement into ready and blocked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Form {
    /// Waiting on a blocker.
    Blocked,
    /// Waiting on a slot: every blocker has settled.
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
    pub id: IssueId,
    pub title: String,
    pub state: State,
    pub form: Form,
    /// The longest path over the blocking edges into this node.
    pub column: usize,
    /// Its place in the depth-first walk of the tree, among the drawn.
    pub row: usize,
    /// The blockers still holding it, as the plan's edges name them: a
    /// container by its own id, an outside issue by its.
    pub blocked_by: Vec<IssueId>,
    /// The nodes its settling helps release: the far ends of the edges
    /// drawn from it, an edge onto a container counted for each leaf
    /// beneath it.
    pub blocks: Vec<IssueId>,
}

/// One arrow: `blocker` settles before `issue` starts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edge {
    pub blocker: IssueId,
    pub issue: IssueId,
}

/// One box: a container beneath the feature, with drawn leaves in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Container {
    pub id: IssueId,
    pub title: String,
    /// How far beneath the feature: its child is 1.
    pub depth: usize,
    /// How many boxes nest inside it, at the deepest.
    pub height: usize,
    /// The nodes inside it, in row order — contiguous rows, by
    /// construction.
    pub leaves: Vec<IssueId>,
}

/// The picture, in grid cells.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Layout {
    /// In row order.
    pub nodes: Vec<Node>,
    /// In (issue row, blocker row) order, each once.
    pub edges: Vec<Edge>,
    /// In tree order, so an outer box comes before the boxes in it.
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
    pub fn node(&self, id: &IssueId) -> Option<&Node> {
        self.nodes.iter().find(|node| &node.id == id)
    }
}

/// The picture of `plan` with every issue in `states` drawn where it
/// stands. Pure: the same plan and states give the same picture.
#[must_use]
pub fn layout(plan: &Plan, states: &BTreeMap<IssueId, State>) -> Layout {
    // The rows: the drawn leaves, in the order the tree's walk visits them.
    let drawn: Vec<&Issue> = plan
        .tree
        .leaves()
        .filter(|leaf| states.contains_key(&leaf.id))
        .collect();
    let row_of: BTreeMap<&IssueId, usize> = drawn
        .iter()
        .enumerate()
        .map(|(row, leaf)| (&leaf.id, row))
        .collect();
    // The rows beneath an id: its own when a drawn leaf, its subtree's
    // when a container, none when settled, outside, or unknown.
    let beneath = |id: &IssueId| -> Vec<usize> {
        plan.tree
            .find_path(|issue| &issue.id == id)
            .map(|path| {
                path.last()
                    .expect("a found path reaches its match")
                    .leaves()
                    .filter_map(|leaf| row_of.get(&leaf.id).copied())
                    .collect()
            })
            .unwrap_or_default()
    };

    // The edges as (issue row, blocker row), blocker side expanded.
    let edges: BTreeSet<(usize, usize)> = plan
        .blocking
        .iter()
        .filter_map(|edge| row_of.get(&edge.issue).map(|&issue| (issue, &edge.blocker)))
        .flat_map(|(issue, blocker)| {
            beneath(blocker)
                .into_iter()
                .map(move |blocker| (issue, blocker))
        })
        .collect();
    let mut into = vec![Vec::new(); drawn.len()];
    let mut blocks = vec![Vec::new(); drawn.len()];
    for &(issue, blocker) in &edges {
        into[issue].push(blocker);
        blocks[blocker].push(drawn[issue].id.clone());
    }
    let mut column = vec![None; drawn.len()];
    for row in 0..drawn.len() {
        longest(row, &into, &mut column, &mut Vec::new());
    }

    let done: BTreeSet<IssueId> = states
        .iter()
        .filter(|(_, state)| matches!(state, State::Merged { .. }))
        .map(|(id, _)| id.clone())
        .collect();
    let ready: BTreeSet<&IssueId> = plan
        .ready(&done)
        .into_iter()
        .map(|issue| &issue.id)
        .collect();
    let blockers = plan.blockers();
    let nodes = drawn
        .iter()
        .enumerate()
        .map(|(row, leaf)| {
            let state = states[&leaf.id].clone();
            Node {
                id: leaf.id.clone(),
                title: leaf.title.clone(),
                form: form(&state, ready.contains(&leaf.id)),
                state,
                column: column[row].unwrap_or(0),
                row,
                blocked_by: blockers
                    .of(&leaf.id)
                    .filter(|blocker| !plan.settled(blocker, &done))
                    .cloned()
                    .collect(),
                blocks: std::mem::take(&mut blocks[row]),
            }
        })
        .collect();

    let mut containers: Vec<Container> = plan
        .tree
        .nodes()
        .filter_map(|issue| {
            let path = plan
                .tree
                .find_path(|candidate| candidate.id == issue.id)
                .expect("a tree's node is in the tree");
            let subtree = path.last().expect("a found path reaches its match");
            if path.len() == 1 || subtree.is_leaf() {
                return None;
            }
            let leaves: Vec<IssueId> = beneath(&issue.id)
                .into_iter()
                .map(|row| drawn[row].id.clone())
                .collect();
            (!leaves.is_empty()).then(|| Container {
                id: issue.id.clone(),
                title: issue.title.clone(),
                depth: path.len() - 1,
                height: 0,
                leaves,
            })
        })
        .collect();
    // A box nests in another when its rows do; the height is how deep
    // the nesting goes.
    let spans: Vec<(usize, usize, usize)> = containers
        .iter()
        .map(|container| {
            let rows = || container.leaves.iter().map(|id| row_of[id]);
            (
                container.depth,
                rows().min().unwrap_or(0),
                rows().max().unwrap_or(0),
            )
        })
        .collect();
    for (container, &(depth, first, last)) in containers.iter_mut().zip(&spans) {
        container.height = spans
            .iter()
            .filter(|&&(inner, from, to)| inner > depth && from >= first && to <= last)
            .map(|&(inner, ..)| inner - depth)
            .max()
            .unwrap_or(0);
    }

    Layout {
        nodes,
        edges: edges
            .into_iter()
            .map(|(issue, blocker)| Edge {
                blocker: drawn[blocker].id.clone(),
                issue: drawn[issue].id.clone(),
            })
            .collect(),
        containers,
    }
}

/// The longest path into `row`: one past the longest into any of its
/// blockers. A blocker already on the path being walked closes a
/// cycle; the edge that closes it contributes nothing — `None`, so the
/// walk ends on any graph and a cycle's members still stand in a row.
fn longest(
    row: usize,
    into: &[Vec<usize>],
    column: &mut [Option<usize>],
    path: &mut Vec<usize>,
) -> Option<usize> {
    if let Some(known) = column[row] {
        return Some(known);
    }
    if path.contains(&row) {
        return None;
    }
    path.push(row);
    let found = into[row]
        .iter()
        .filter_map(|&blocker| longest(blocker, into, column, path))
        .map(|column| column + 1)
        .max()
        .unwrap_or(0);
    path.pop();
    column[row] = Some(found);
    Some(found)
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

#[cfg(test)]
mod tests {
    use super::super::fixtures::{id, nine_waiting_on_container_seven, node, plan, two_then_three};
    use super::*;

    /// The states a build starts with: every issue of work, Waiting.
    fn waiting(plan: &Plan) -> BTreeMap<IssueId, State> {
        plan.work(&BTreeSet::new())
            .into_iter()
            .map(|issue| (issue.id.clone(), State::Waiting))
            .collect()
    }

    /// (id, column, row) for every node, in row order.
    fn cells(layout: &Layout) -> Vec<(u64, usize, usize)> {
        layout
            .nodes
            .iter()
            .map(|node| (node.id.0.parse().unwrap(), node.column, node.row))
            .collect()
    }

    fn arrows(layout: &Layout) -> Vec<(u64, u64)> {
        layout
            .edges
            .iter()
            .map(|edge| {
                (
                    edge.blocker.0.parse().unwrap(),
                    edge.issue.0.parse().unwrap(),
                )
            })
            .collect()
    }

    fn numbers(ids: &[IssueId]) -> Vec<u64> {
        ids.iter().map(|id| id.0.parse().unwrap()).collect()
    }

    #[test]
    fn a_one_level_plan_lays_its_order_out_left_to_right() {
        let plan = two_then_three();
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(cells(&layout), [(2, 0, 0), (3, 1, 1)]);
        assert_eq!(arrows(&layout), [(2, 3)]);
        assert!(
            layout.containers.is_empty(),
            "the feature's box is the pane"
        );
        assert_eq!((layout.columns(), layout.rows()), (2, 2));
        let three = layout.node(&id(3)).unwrap();
        assert_eq!(three.form, Form::Blocked);
        assert_eq!(numbers(&three.blocked_by), [2]);
        assert_eq!(numbers(&layout.node(&id(2)).unwrap().blocks), [3]);
        assert_eq!(layout.node(&id(2)).unwrap().form, Form::Ready);
    }

    /// 1 holds 2 and 7; 7 holds 3 and 8; 8 holds 4. No order, so one
    /// column; the tree alone places the rows, and boxes nest.
    #[test]
    fn a_nested_sub_feature_is_a_box_within_a_box() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2, 7], &[]),
                node(2, false, &[], &[]),
                node(7, false, &[3, 8], &[]),
                node(3, false, &[], &[]),
                node(8, false, &[4], &[]),
                node(4, false, &[], &[]),
            ],
        );
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(cells(&layout), [(2, 0, 0), (3, 0, 1), (4, 0, 2)]);
        assert!(layout.edges.is_empty());
        let boxes: Vec<(u64, usize, usize, Vec<u64>)> = layout
            .containers
            .iter()
            .map(|container| {
                (
                    container.id.0.parse().unwrap(),
                    container.depth,
                    container.height,
                    numbers(&container.leaves),
                )
            })
            .collect();
        assert_eq!(boxes, [(7, 1, 1, vec![3, 4]), (8, 2, 0, vec![4])]);
    }

    /// 9 waits on the container 7: an arrow from each of 7's leaves.
    #[test]
    fn an_edge_onto_a_container_is_an_edge_from_each_leaf_beneath_it() {
        let plan = nine_waiting_on_container_seven();
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(cells(&layout), [(2, 0, 0), (8, 0, 1), (9, 1, 2)]);
        assert_eq!(arrows(&layout), [(2, 9), (8, 9)]);
        let nine = layout.node(&id(9)).unwrap();
        assert_eq!(
            numbers(&nine.blocked_by),
            [7],
            "held by the container, in the plan's own words"
        );
        assert_eq!(numbers(&layout.node(&id(2)).unwrap().blocks), [9]);
        assert_eq!(layout.containers.len(), 1);
        assert_eq!(numbers(&layout.containers[0].leaves), [2, 8]);
    }

    #[test]
    fn a_diamond_puts_its_arms_in_one_column_and_the_join_past_them() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
                node(4, false, &[], &[(2, false)]),
                node(5, false, &[], &[(3, false), (4, false)]),
            ],
        );
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(cells(&layout), [(2, 0, 0), (3, 1, 1), (4, 1, 2), (5, 2, 3)]);
        assert_eq!(arrows(&layout), [(2, 3), (2, 4), (3, 5), (4, 5)]);
        assert_eq!(numbers(&layout.node(&id(5)).unwrap().blocked_by), [3, 4]);
    }

    /// 7 holds 2, a closed 3, and 9; 8 holds 4 and 5; 6 stands alone —
    /// with an order that puts 2 far right. The box for 7 holds 2 and
    /// 9 on adjacent rows, the closed leaf squeezed out, and no other
    /// node shares those rows however the columns fall.
    #[test]
    fn a_containers_box_encloses_exactly_its_own_leaves() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[7, 8, 6], &[]),
                node(7, false, &[2, 3, 9], &[]),
                node(2, false, &[], &[(4, false), (6, false)]),
                node(3, true, &[], &[]),
                node(9, false, &[], &[]),
                node(8, false, &[4, 5], &[]),
                node(4, false, &[], &[(5, false)]),
                node(5, false, &[], &[]),
                node(6, false, &[], &[]),
            ],
        );
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(
            cells(&layout),
            [(2, 2, 0), (9, 0, 1), (4, 1, 2), (5, 0, 3), (6, 0, 4)],
            "3 is settled and takes no row"
        );
        for container in &layout.containers {
            let rows: Vec<usize> = container
                .leaves
                .iter()
                .map(|id| layout.node(id).unwrap().row)
                .collect();
            let first = rows[0];
            assert_eq!(
                rows,
                (first..first + rows.len()).collect::<Vec<_>>(),
                "{}'s rows are contiguous",
                container.id
            );
            let inside: Vec<&IssueId> = layout
                .nodes
                .iter()
                .filter(|node| (first..first + rows.len()).contains(&node.row))
                .map(|node| &node.id)
                .collect();
            assert_eq!(
                inside,
                container.leaves.iter().collect::<Vec<_>>(),
                "{}'s box holds nothing else",
                container.id
            );
        }
        assert_eq!(layout.containers.len(), 2);
    }

    #[test]
    fn what_is_not_work_is_not_drawn() {
        // 5 is a closed container over an open 6: an abandoned subtree.
        // 3 waits on 6, which the plan knows and will never settle.
        let plan = plan(
            1,
            vec![
                node(1, false, &[3, 5], &[]),
                node(3, false, &[], &[(6, false)]),
                node(5, true, &[6], &[]),
                node(6, false, &[], &[]),
            ],
        );
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(cells(&layout), [(3, 0, 0)]);
        assert!(layout.edges.is_empty(), "no node to draw the arrow from");
        assert!(layout.containers.is_empty(), "an empty box is no box");
        let three = layout.node(&id(3)).unwrap();
        assert_eq!(three.form, Form::Blocked);
        assert_eq!(numbers(&three.blocked_by), [6], "held all the same");
    }

    #[test]
    fn an_outside_or_unknown_blocker_holds_the_leaf_but_takes_no_column() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2], &[]),
                node(2, false, &[], &[(55, false)]),
            ],
        );
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(cells(&layout), [(2, 0, 0)]);
        assert_eq!(numbers(&layout.node(&id(2)).unwrap().blocked_by), [55]);
    }

    #[test]
    fn a_cycle_still_lays_out() {
        let plan = super::super::fixtures::two_and_three_in_a_cycle();
        let layout = layout(&plan, &waiting(&plan));
        assert_eq!(cells(&layout), [(2, 1, 0), (3, 0, 1)]);
        assert_eq!(arrows(&layout), [(3, 2), (2, 3)]);
    }

    #[test]
    fn the_form_follows_the_state_and_the_plans_judgement() {
        let plan = two_then_three();
        let merged = State::Merged {
            commit: "abc".to_owned(),
            checked: true,
        };
        let states = [(id(2), merged), (id(3), State::Waiting)].into();
        let landed = layout(&plan, &states);
        assert_eq!(landed.node(&id(2)).unwrap().form, Form::Merged);
        let three = landed.node(&id(3)).unwrap();
        assert_eq!(three.form, Form::Ready, "2 landing released 3");
        assert!(three.blocked_by.is_empty());

        let states = [
            (id(2), State::Running),
            (
                id(3),
                State::Skipped {
                    reason: "2 failed".to_owned(),
                },
            ),
        ]
        .into();
        let lost = layout(&plan, &states);
        assert_eq!(lost.node(&id(2)).unwrap().form, Form::Running);
        assert_eq!(lost.node(&id(3)).unwrap().form, Form::Skipped);
    }
}
