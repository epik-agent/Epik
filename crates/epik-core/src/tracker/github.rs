//! GitHub as an issue tracker: one repository's issues, their open or
//! closed state, and the blocked-by edges among them as the order.
//!
//! An issue's id is its number, since a bare `154` names an issue only
//! inside one repository, and this tracker holds that repository. The
//! dependents of an issue are the issues GitHub says it blocks,
//! followed outward until no more are found, each after the members
//! blocking it: what [`GitHub::issue_graph`] reads, one issue at a
//! time, folded by [`closure`].

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow};

use super::{IssueId, IssueTracker};
use crate::dag::{Dag, Node};
use crate::github::{GitHub, IssueGraph, Repo, State};

/// One repository's issues, read and written through `github`.
#[derive(Clone, Debug)]
pub struct GitHubIssues {
    pub github: GitHub,
    pub repo: Repo,
}

impl GitHubIssues {
    /// The number behind an id: GitHub numbers its issues, and an id
    /// that is not a number belongs to some other tracker.
    fn number(id: &str) -> Result<u64> {
        id.parse()
            .map_err(|_| anyhow!("{id} is not a GitHub issue number"))
    }
}

impl IssueTracker<State> for GitHubIssues {
    /// The title as a heading, then the body.
    fn content(&self, id: &str) -> Result<String> {
        let issue = self.github.issue(&self.repo, Self::number(id)?)?;
        Ok(match issue.body.trim() {
            "" => format!("# {}", issue.title),
            body => format!("# {}\n\n{body}", issue.title),
        })
    }

    fn get_status(&self, id: &str) -> Result<State> {
        Ok(self.github.issue(&self.repo, Self::number(id)?)?.state)
    }

    fn set_status(&mut self, id: &str, status: State) -> Result<()> {
        let number = Self::number(id)?;
        match status {
            State::Open => self.github.reopen_issue(&self.repo, number)?,
            State::Closed => self.github.close_issue(&self.repo, number)?,
        };
        Ok(())
    }

    fn dependents(&self, root: &str) -> Result<Dag<IssueId>> {
        closure(Self::number(root)?, |number| {
            self.github
                .issue_graph(&self.repo, number)
                .with_context(|| format!("reading issue {number}"))
        })
    }
}

/// `root` and every issue it blocks, transitively, each graph read by
/// `fetch` once, in the order found; each after the members blocking
/// it.
fn closure(root: u64, fetch: impl Fn(u64) -> Result<IssueGraph>) -> Result<Dag<IssueId>> {
    // The graphs in discovery order, and each number's position among
    // them once read.
    let mut graphs: Vec<IssueGraph> = Vec::new();
    let mut seen: BTreeMap<u64, usize> = BTreeMap::new();
    let mut queue = vec![root];
    while let Some(number) = queue.pop() {
        if seen.contains_key(&number) {
            continue;
        }
        let graph = fetch(number)?;
        queue.extend(graph.blocking.iter().rev().map(|edge| edge.number));
        seen.insert(number, graphs.len());
        graphs.push(graph);
    }
    Dag::new(
        graphs
            .iter()
            .map(|graph| {
                Node::new(
                    graph.issue.number.to_string(),
                    graph
                        .blocked_by
                        .iter()
                        .filter_map(|edge| seen.get(&edge.number).copied()),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Edge, Error, Issue};

    fn edge(number: u64) -> Edge {
        Edge {
            number,
            title: format!("issue {number}"),
            state: State::Open,
        }
    }

    /// A graph for `number`: what blocks it, and what it blocks.
    fn graph(number: u64, blocked_by: &[u64], blocking: &[u64]) -> IssueGraph {
        IssueGraph {
            issue: Issue {
                number,
                title: format!("issue {number}"),
                body: String::new(),
                state: State::Open,
            },
            sub_issues: vec![],
            blocked_by: blocked_by.iter().copied().map(edge).collect(),
            blocking: blocking.iter().copied().map(edge).collect(),
        }
    }

    fn ids(dag: &Dag<IssueId>) -> Vec<&str> {
        dag.nodes().map(|node| node.value().as_str()).collect()
    }

    fn fetching(graphs: Vec<IssueGraph>) -> impl Fn(u64) -> Result<IssueGraph> {
        move |number| {
            graphs
                .iter()
                .find(|graph| graph.issue.number == number)
                .cloned()
                .ok_or_else(|| anyhow!("no issue {number}"))
        }
    }

    #[test]
    fn the_closure_is_the_root_and_what_it_blocks_each_after_its_blockers() {
        // 1 blocks 2 and 3; 3 also waits on 2; 4 blocks 3 from outside
        // the closure and is not a member.
        let dag = closure(
            1,
            fetching(vec![
                graph(1, &[], &[2, 3]),
                graph(2, &[1], &[3]),
                graph(3, &[1, 2, 4], &[]),
                graph(4, &[], &[3]),
            ]),
        )
        .unwrap();
        assert_eq!(ids(&dag), ["1", "2", "3"]);
        let three = dag.find(|id| id == "3").unwrap();
        let before: Vec<&str> = three
            .predecessors()
            .map(|node| node.value().as_str())
            .collect();
        assert_eq!(before, ["1", "2"], "the outside blocker is no predecessor");
    }

    #[test]
    fn a_leaf_is_its_own_closure_and_each_graph_is_read_once() {
        let reads = std::cell::Cell::new(0);
        let dag = closure(7, |number| {
            reads.set(reads.get() + 1);
            Ok(graph(number, &[], &[]))
        })
        .unwrap();
        assert_eq!(ids(&dag), ["7"]);
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn a_fetch_that_fails_is_the_closures_failure() {
        let error = closure(1, |_| Err(anyhow!("offline"))).unwrap_err();
        assert_eq!(error.to_string(), "offline");
    }

    fn tracker() -> GitHubIssues {
        // Port 1 is reserved; a verb that reached the wire would say so.
        GitHubIssues {
            github: GitHub::at("http://127.0.0.1:1", None),
            repo: Repo::new("epik-agent", "Epik"),
        }
    }

    #[test]
    fn an_id_from_another_tracker_is_refused_in_words() {
        let error = tracker().dependents("EPK-12").unwrap_err();
        assert_eq!(error.to_string(), "EPK-12 is not a GitHub issue number");
    }

    #[test]
    fn the_verbs_that_need_a_token_refuse_before_any_wire_is_touched() {
        let mut tracker = tracker();
        for error in [
            tracker.dependents("1").unwrap_err(),
            tracker.set_status("1", State::Closed).unwrap_err(),
            tracker.set_status("1", State::Open).unwrap_err(),
        ] {
            assert!(
                error.root_cause().downcast_ref::<Error>() == Some(&Error::TokenAbsent),
                "{error:#}"
            );
        }
    }
}
