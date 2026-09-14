//! A tracker read from one JSON file: a list of issues, each with an
//! id, a title, a body, a status, and the ids it depends on. Statuses
//! are plain strings.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow, bail};
use epik_core::dag::{Dag, Node};
use epik_core::tracker::{IssueId, IssueTracker};
use serde::Deserialize;

/// The issues of one JSON file, in the file's order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(transparent)]
pub struct JsonTracker {
    issues: Vec<Issue>,
}

/// One issue as the file spells it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct Issue {
    id: IssueId,
    title: String,
    body: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    depends_on: Vec<IssueId>,
}

impl JsonTracker {
    /// The tracker `json` spells.
    ///
    /// # Errors
    ///
    /// JSON of another shape, an id used twice, or a dependency on an
    /// issue the file does not hold.
    pub fn read(json: &str) -> Result<Self> {
        let tracker: Self = serde_json::from_str(json)?;
        let mut seen = BTreeSet::new();
        for issue in &tracker.issues {
            if !seen.insert(&issue.id) {
                bail!("issue {} appears twice", issue.id);
            }
        }
        for issue in &tracker.issues {
            for dependency in &issue.depends_on {
                if !seen.contains(dependency) {
                    bail!(
                        "issue {} depends on {dependency}, which is not an issue",
                        issue.id
                    );
                }
            }
        }
        Ok(tracker)
    }

    fn issue(&self, id: &str) -> Result<&Issue> {
        self.issues
            .iter()
            .find(|issue| issue.id == id)
            .ok_or_else(|| anyhow!("there is no issue {id}"))
    }

    fn issue_mut(&mut self, id: &str) -> Result<&mut Issue> {
        self.issues
            .iter_mut()
            .find(|issue| issue.id == id)
            .ok_or_else(|| anyhow!("there is no issue {id}"))
    }
}

impl IssueTracker<String> for JsonTracker {
    fn content(&self, id: &str) -> Result<String> {
        Ok(self.issue(id)?.body.clone())
    }

    fn get_status(&self, id: &str) -> Result<String> {
        Ok(self.issue(id)?.status.clone())
    }

    fn set_status(&mut self, id: &str, status: String) -> Result<()> {
        self.issue_mut(id)?.status = status;
        Ok(())
    }

    /// The closure of `root` under "depends on it", in the file's
    /// order, each issue after the members it depends on.
    fn dependents(&self, root: &str) -> Result<Dag<IssueId>> {
        self.issue(root)?;
        let mut members: BTreeSet<&str> = BTreeSet::from([root]);
        loop {
            let more: Vec<&str> = self
                .issues
                .iter()
                .filter(|issue| !members.contains(issue.id.as_str()))
                .filter(|issue| {
                    issue
                        .depends_on
                        .iter()
                        .any(|dependency| members.contains(dependency.as_str()))
                })
                .map(|issue| issue.id.as_str())
                .collect();
            if more.is_empty() {
                break;
            }
            members.extend(more);
        }
        let ordered: Vec<&Issue> = self
            .issues
            .iter()
            .filter(|issue| members.contains(issue.id.as_str()))
            .collect();
        let position = |id: &str| ordered.iter().position(|issue| issue.id == id);
        Dag::new(
            ordered
                .iter()
                .map(|issue| {
                    Node::new(
                        issue.id.clone(),
                        issue
                            .depends_on
                            .iter()
                            .filter_map(|dependency| position(dependency)),
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDER: &str = include_str!("../fixtures/adder/issues.json");

    fn adder() -> JsonTracker {
        JsonTracker::read(ADDER).unwrap()
    }

    fn ids(dag: &Dag<IssueId>) -> Vec<&str> {
        dag.nodes().map(|node| node.value().as_str()).collect()
    }

    #[test]
    fn the_adder_fixture_reads_and_answers_for_its_issues() {
        let mut tracker = adder();
        assert_eq!(
            tracker.content("1").unwrap(),
            "pyproject.toml\n.python-version"
        );
        assert_eq!(tracker.get_status("2").unwrap(), "open");
        tracker.set_status("2", "done".to_owned()).unwrap();
        assert_eq!(tracker.get_status("2").unwrap(), "done");
        assert_eq!(
            tracker.content("9").unwrap_err().to_string(),
            "there is no issue 9"
        );
    }

    #[test]
    fn the_dependents_of_the_root_are_the_whole_project_in_order() {
        let dag = adder().dependents("1").unwrap();
        assert_eq!(ids(&dag), ["1", "2", "3"]);
        let root = dag.find(|id| id == "1").unwrap();
        assert_eq!(root.predecessors().count(), 0);
        for id in ["2", "3"] {
            let issue = dag.find(|found| found == id).unwrap();
            let before: Vec<&str> = issue
                .predecessors()
                .map(|node| node.value().as_str())
                .collect();
            assert_eq!(before, ["1"], "{id} waits on the scaffolding");
        }
    }

    #[test]
    fn a_leaf_has_only_itself_as_dependents() {
        assert_eq!(ids(&adder().dependents("3").unwrap()), ["3"]);
    }

    #[test]
    fn dependents_reach_through_a_chain_and_keep_the_order_within_the_closure() {
        let tracker = JsonTracker::read(
            r#"[
                {"id": "c", "title": "", "body": "", "depends_on": ["b"]},
                {"id": "a", "title": "", "body": ""},
                {"id": "b", "title": "", "body": "", "depends_on": ["a"]},
                {"id": "x", "title": "", "body": ""}
            ]"#,
        )
        .unwrap();
        let dag = tracker.dependents("a").unwrap();
        assert_eq!(ids(&dag), ["a", "b", "c"], "x is nobody's dependent");
    }

    #[test]
    fn a_file_that_does_not_hold_together_is_refused_in_words() {
        let twice =
            r#"[{"id": "1", "title": "", "body": ""}, {"id": "1", "title": "", "body": ""}]"#;
        assert_eq!(
            JsonTracker::read(twice).unwrap_err().to_string(),
            "issue 1 appears twice"
        );
        let dangling = r#"[{"id": "1", "title": "", "body": "", "depends_on": ["2"]}]"#;
        assert_eq!(
            JsonTracker::read(dangling).unwrap_err().to_string(),
            "issue 1 depends on 2, which is not an issue"
        );
        let cycle = r#"[
            {"id": "1", "title": "", "body": "", "depends_on": ["2"]},
            {"id": "2", "title": "", "body": "", "depends_on": ["1"]}
        ]"#;
        let error = JsonTracker::read(cycle)
            .unwrap()
            .dependents("1")
            .unwrap_err()
            .to_string();
        assert!(error.contains("cycle"), "{error}");
    }
}
