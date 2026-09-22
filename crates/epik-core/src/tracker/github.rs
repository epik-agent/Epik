//! GitHub as an issue tracker: one repository's issues, their open or
//! closed state, their sub-issue edges as containment, and the
//! blocked-by edges among them as the order.
//!
//! An issue's id is its number, which names an issue only inside one
//! repository, so this tracker holds that repository. The work under
//! an issue is what [`GitHub::issue_graph`] reads, one open member at
//! a time, folded by [`work`](super::work). GitHub lets both edge
//! kinds leave the repository they start in; [`member`] is where an
//! edge that does is read as foreign rather than as a number here.

use anyhow::{Context, Result, bail};

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
        member(&self.repo, &graph)
    }
}

/// The graph as the shape of a plan reads it, in `repo`, whose numbers
/// are the ids a plan travels on.
///
/// An edge may leave `repo`, and a number alone would read there as
/// `repo`'s own issue of that number. Containment that leaves `repo` is
/// refused whatever its state: the issue beneath it is not this
/// repository's to plan, fetch or close. Order that leaves `repo` is
/// refused while it is open and satisfied once closed, as a blocker
/// outside the feature is.
///
/// # Errors
///
/// A sub-issue outside `repo`, or an open blocker outside it, each in
/// words naming the foreign issue.
fn member(repo: &Repo, graph: &IssueGraph) -> Result<Member<u64>> {
    let number = graph.issue.number;
    if let Some(foreign) = graph.sub_issues.iter().find(|edge| !edge.within(repo)) {
        bail!(
            "issue {number} contains {}, which is outside {repo}",
            foreign.spelled(repo)
        );
    }
    let open = |edge: &Edge| edge.state == State::Open;
    if let Some(foreign) = graph
        .blocked_by
        .iter()
        .find(|edge| !edge.within(repo) && open(edge))
    {
        bail!(
            "issue {number} is blocked by {}, which is open and outside {repo}",
            foreign.spelled(repo)
        );
    }
    let related = |edges: &[Edge]| {
        edges
            .iter()
            .filter(|edge| edge.within(repo))
            .map(|edge| Related {
                id: edge.number,
                open: open(edge),
            })
            .collect()
    };
    Ok(Member {
        open: graph.issue.state == State::Open,
        sub_issues: related(&graph.sub_issues),
        blocked_by: related(&graph.blocked_by),
    })
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
            repository: None,
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
            member(&Repo::new("epik-agent", "Epik"), &graph).unwrap(),
            Member {
                open: false,
                sub_issues: vec![related(4, true), related(5, false)],
                blocked_by: vec![related(2, false)],
            }
        );
    }

    /// An edge to issue `number` of another repository.
    fn foreign(number: u64, state: State) -> Edge {
        Edge {
            repository: Some(Repo::new("other", "repo")),
            ..edge(number, state)
        }
    }

    fn graph(number: u64, sub_issues: Vec<Edge>, blocked_by: Vec<Edge>) -> IssueGraph {
        IssueGraph {
            issue: Issue {
                number,
                title: format!("issue {number}"),
                body: String::new(),
                state: State::Open,
            },
            sub_issues,
            blocked_by,
            blocking: vec![],
        }
    }

    #[test]
    fn an_edge_naming_no_repository_is_this_repositorys_own() {
        let graph = graph(1, vec![edge(2, State::Open)], vec![edge(3, State::Open)]);
        let member = member(&Repo::new("epik-agent", "Epik"), &graph).unwrap();
        assert_eq!(member.sub_issues, [Related { id: 2, open: true }]);
        assert_eq!(member.blocked_by, [Related { id: 3, open: true }]);
    }

    #[test]
    fn an_open_blocker_in_another_repository_refuses_and_names_it() {
        let graph = graph(1, vec![], vec![foreign(12, State::Open)]);
        let error = member(&Repo::new("epik-agent", "Epik"), &graph).unwrap_err();
        assert_eq!(
            error.to_string(),
            "issue 1 is blocked by other/repo#12, which is open and outside epik-agent/Epik"
        );
    }

    #[test]
    fn a_closed_blocker_in_another_repository_is_satisfied_and_dropped() {
        let graph = graph(1, vec![], vec![foreign(12, State::Closed)]);
        let member = member(&Repo::new("epik-agent", "Epik"), &graph).unwrap();
        assert_eq!(member.blocked_by, [], "a closed blocker is satisfied");
    }

    #[test]
    fn a_sub_issue_in_another_repository_refuses_whatever_its_state() {
        for state in [State::Open, State::Closed] {
            let graph = graph(1, vec![foreign(12, state)], vec![]);
            let error = member(&Repo::new("epik-agent", "Epik"), &graph).unwrap_err();
            assert_eq!(
                error.to_string(),
                "issue 1 contains other/repo#12, which is outside epik-agent/Epik"
            );
        }
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
