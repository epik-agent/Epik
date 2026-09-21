//! GitHub as an issue tracker: one repository's issues, their open or
//! closed state, their sub-issue edges as containment, and the
//! blocked-by edges among them as the order.
//!
//! An issue's id is its number, which names an issue only inside one
//! repository, so this tracker holds that repository. The work under
//! an issue is what [`GitHub::issue_graph`] reads, one open member at
//! a time, folded by [`work`](super::work).

use anyhow::{Context, Result};

use super::{IssueTracker, Member, Related};
use crate::github::{Edge, GitHub, IssueGraph, Repo, State};

/// One repository's issues, read and written through `github`.
#[derive(Clone, Debug)]
pub struct GitHubIssues {
    pub github: GitHub,
    pub repo: Repo,
}

impl IssueTracker<u64, State> for GitHubIssues {
    /// The title as a heading, then the body.
    fn content(&self, number: &u64) -> Result<String> {
        let issue = self.github.issue(&self.repo, *number)?;
        Ok(match issue.body.trim() {
            "" => format!("# {}", issue.title),
            body => format!("# {}\n\n{body}", issue.title),
        })
    }

    fn get_status(&self, number: &u64) -> Result<State> {
        Ok(self.github.issue(&self.repo, *number)?.state)
    }

    fn set_status(&mut self, number: &u64, status: State) -> Result<()> {
        match status {
            State::Open => self.github.reopen_issue(&self.repo, *number)?,
            State::Closed => self.github.close_issue(&self.repo, *number)?,
        };
        Ok(())
    }

    fn member(&self, number: &u64) -> Result<Member<u64>> {
        let graph = self
            .github
            .issue_graph(&self.repo, *number)
            .with_context(|| format!("reading issue {number}"))?;
        Ok(member(&graph))
    }
}

/// The graph as the shape of a plan reads it.
fn member(graph: &IssueGraph) -> Member<u64> {
    let related = |edges: &[Edge]| {
        edges
            .iter()
            .map(|edge| Related {
                id: edge.number,
                open: edge.state == State::Open,
            })
            .collect()
    };
    Member {
        open: graph.issue.state == State::Open,
        sub_issues: related(&graph.sub_issues),
        blocked_by: related(&graph.blocked_by),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Error, Issue};

    fn edge(number: u64, state: State) -> Edge {
        Edge {
            number,
            title: format!("issue {number}"),
            state,
        }
    }

    #[test]
    fn a_graph_reads_as_a_member_by_its_state_its_sub_issues_and_its_blockers() {
        let graph = IssueGraph {
            issue: Issue {
                number: 3,
                title: "issue 3".to_owned(),
                body: String::new(),
                state: State::Closed,
            },
            sub_issues: vec![edge(4, State::Open), edge(5, State::Closed)],
            blocked_by: vec![edge(2, State::Closed)],
            blocking: vec![edge(6, State::Open)],
        };
        let related = |id, open| Related { id, open };
        assert_eq!(
            member(&graph),
            Member {
                open: false,
                sub_issues: vec![related(4, true), related(5, false)],
                blocked_by: vec![related(2, false)],
            }
        );
    }

    fn tracker() -> GitHubIssues {
        // Port 1 is reserved; a verb that reached the wire would say so.
        GitHubIssues {
            github: GitHub::at("http://127.0.0.1:1", None),
            repo: Repo::new("epik-agent", "Epik"),
        }
    }

    #[test]
    fn the_verbs_that_need_a_token_refuse_before_any_wire_is_touched() {
        let mut tracker = tracker();
        for error in [
            tracker.work(&1).unwrap_err(),
            tracker.set_status(&1, State::Closed).unwrap_err(),
            tracker.set_status(&1, State::Open).unwrap_err(),
        ] {
            assert_eq!(
                error.root_cause().downcast_ref::<Error>(),
                Some(&Error::TokenAbsent),
                "{error:#}"
            );
        }
    }
}
