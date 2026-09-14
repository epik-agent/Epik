//! GitHub as a tracker: the issue verbs, and only those, and the
//! reading of an issue's graph in the plan's vocabulary.

use super::Tracker;
use crate::feature::{self, Feature, IssueId, Plan};
use crate::github::{GitHub, IssueGraph, Repo, State};

/// The [`Tracker`] seam, spoken by GitHub: issue verbs only, each one of
/// this module's verbs with its errors rendered in words a model reads.
/// It holds the repository, because a bare [`IssueId`] — `154` — names an
/// issue only inside one.
#[derive(Debug)]
pub struct GitHubTracker<'a> {
    github: &'a GitHub,
    repo: Repo,
}

impl<'a> GitHubTracker<'a> {
    #[must_use]
    pub const fn new(github: &'a GitHub, repo: Repo) -> Self {
        Self { github, repo }
    }

    /// The number behind an id. GitHub numbers its issues; an id that is
    /// not a number belongs to some other tracker.
    fn number(id: &IssueId) -> Result<u64, String> {
        id.0.parse()
            .map_err(|_| format!("{id} is not a GitHub issue number"))
    }
}

impl Tracker for GitHubTracker<'_> {
    fn plan(&self, feature: &Feature) -> Result<Plan, String> {
        Plan::descend(feature, |id| {
            self.github
                .issue_graph(&self.repo, Self::number(id)?)
                .map(fetched)
                .map_err(|error| error.to_string())
        })
    }

    fn note(&self, issue: &IssueId, body: &str) -> Result<(), String> {
        self.github
            .comment(&self.repo, Self::number(issue)?, body)
            .map_err(|error| error.to_string())
    }

    fn close(&self, issue: &IssueId) -> Result<(), String> {
        self.github
            .close_issue(&self.repo, Self::number(issue)?)
            .map(drop)
            .map_err(|error| error.to_string())
    }
}

/// An [`IssueGraph`] in the plan's vocabulary: numbers become ids, the
/// edges keep title and state, and the body stays behind — a plan carries
/// what schedules and renders, nothing more.
fn fetched(graph: IssueGraph) -> feature::Node {
    feature::Node {
        issue: domain(graph.issue.number, graph.issue.title, graph.issue.state),
        children: graph
            .sub_issues
            .iter()
            .map(|edge| IssueId::from(edge.number))
            .collect(),
        blockers: graph
            .blocked_by
            .into_iter()
            .map(|edge| domain(edge.number, edge.title, edge.state))
            .collect(),
    }
}

fn domain(number: u64, title: String, state: State) -> feature::Issue {
    feature::Issue {
        id: IssueId::from(number),
        title,
        closed: state == State::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Edge, Issue};

    fn edge(number: u64, state: State) -> Edge {
        Edge {
            number,
            title: format!("issue {number}"),
            state,
        }
    }

    fn graph(number: u64, sub_issues: Vec<Edge>, blocked_by: Vec<Edge>) -> IssueGraph {
        IssueGraph {
            issue: Issue {
                number,
                title: format!("issue {number}"),
                body: "the body stays behind".to_owned(),
                state: State::Open,
            },
            sub_issues,
            blocked_by,
        }
    }

    #[test]
    fn a_graph_speaks_the_plans_vocabulary() {
        let node = fetched(graph(
            102,
            vec![edge(105, State::Open), edge(106, State::Closed)],
            vec![],
        ));
        assert_eq!(node.issue.id, IssueId::from(102));
        assert!(!node.issue.closed);
        assert_eq!(node.children, [IssueId::from(105), IssueId::from(106)]);
        assert!(node.blockers.is_empty());
    }

    #[test]
    fn a_blocker_carries_the_state_that_decides_whether_it_holds() {
        let node = fetched(graph(106, vec![], vec![edge(105, State::Open)]));
        assert!(node.children.is_empty());
        assert_eq!(node.blockers.len(), 1);
        assert_eq!(node.blockers[0].id, IssueId::from(105));
        assert!(!node.blockers[0].closed);
        let closed = fetched(graph(106, vec![], vec![edge(105, State::Closed)]));
        assert!(closed.blockers[0].closed);
    }

    #[test]
    fn the_tracker_renders_a_github_refusal_in_words() {
        // No token, so GraphQL refuses before any wire is touched — and
        // the tracker's answer is that refusal as a sentence, not a value
        // the model cannot read.
        let github = GitHub::at("http://127.0.0.1:1", None);
        let tracker = GitHubTracker::new(&github, Repo::new("epik-agent", "Epik"));
        let plan = tracker.plan(&Feature(IssueId::from(154))).unwrap_err();
        assert!(plan.contains("Settings (Cmd+,)"), "{plan}");
        let note = tracker.note(&IssueId::from(154), "hi").unwrap_err();
        assert!(note.contains("Settings (Cmd+,)"), "{note}");
        let close = tracker.close(&IssueId::from(154)).unwrap_err();
        assert!(close.contains("Settings (Cmd+,)"), "{close}");
    }

    #[test]
    fn an_id_from_another_tracker_is_refused_in_words() {
        let github = GitHub::at("http://127.0.0.1:1", None);
        let tracker = GitHubTracker::new(&github, Repo::new("epik-agent", "Epik"));
        let error = tracker.plan(&Feature(IssueId::from("EPK-12"))).unwrap_err();
        assert!(error.contains("EPK-12"), "{error}");
        assert!(error.contains("not a GitHub issue number"), "{error}");
    }
}
