//! GitHub as a tracker: the issue verbs, and only those.

use super::Tracker;
use crate::feature::{Feature, IssueId, Plan};
use crate::github::{GitHub, Repo, fetched};

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

#[cfg(test)]
mod tests {
    use super::*;

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
