//! A tracker read from one JSON file: a list of issues, each with an
//! id, a title, a body, a status, the ids it contains, and the ids it
//! depends on. Ids and statuses are plain strings, and an issue is
//! open until its status is `closed`.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow, bail};
use epik_core::tracker::{IssueTracker, Member, Related};
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
    id: String,
    title: String,
    body: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    sub_issues: Vec<String>,
    #[serde(default)]
    depends_on: Vec<String>,
}

impl Issue {
    fn open(&self) -> bool {
        self.status != "closed"
    }
}

impl JsonTracker {
    /// The tracker `json` spells.
    ///
    /// # Errors
    ///
    /// JSON of another shape, an id used twice, or a sub-issue or a
    /// dependency the file does not hold.
    pub fn read(json: &str) -> Result<Self> {
        let tracker: Self = serde_json::from_str(json)?;
        let mut seen = BTreeSet::new();
        for issue in &tracker.issues {
            if !seen.insert(&issue.id) {
                bail!("issue {} appears twice", issue.id);
            }
        }
        for issue in &tracker.issues {
            for (verb, ids) in [
                ("contains", &issue.sub_issues),
                ("depends on", &issue.depends_on),
            ] {
                if let Some(id) = ids.iter().find(|id| !seen.contains(id)) {
                    bail!("issue {} {verb} {id}, which is not an issue", issue.id);
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

    /// The issues `ids` name, each with whether it is open.
    fn related(&self, ids: &[String]) -> Result<Vec<Related<String>>> {
        ids.iter()
            .map(|id| {
                Ok(Related {
                    id: id.clone(),
                    open: self.issue(id)?.open(),
                })
            })
            .collect()
    }

    fn issue_mut(&mut self, id: &str) -> Result<&mut Issue> {
        self.issues
            .iter_mut()
            .find(|issue| issue.id == id)
            .ok_or_else(|| anyhow!("there is no issue {id}"))
    }
}

impl IssueTracker<String, String> for JsonTracker {
    fn content(&self, id: &String) -> Result<String> {
        Ok(self.issue(id)?.body.clone())
    }

    fn get_status(&self, id: &String) -> Result<String> {
        Ok(self.issue(id)?.status.clone())
    }

    fn set_status(&mut self, id: &String, status: String) -> Result<()> {
        self.issue_mut(id)?.status = status;
        Ok(())
    }

    /// The file's `sub_issues` as containment and its `depends_on`
    /// as blocked-by.
    fn member(&self, id: &String) -> Result<Member<String>> {
        let issue = self.issue(id)?;
        Ok(Member {
            open: issue.open(),
            sub_issues: self.related(&issue.sub_issues)?,
            blocked_by: self.related(&issue.depends_on)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use epik_core::dag::Dag;

    const ADDER: &str = include_str!("../fixtures/adder/issues.json");

    fn adder() -> JsonTracker {
        JsonTracker::read(ADDER).unwrap()
    }

    fn id(id: &str) -> String {
        id.to_owned()
    }

    #[test]
    fn the_adder_fixture_reads_and_answers_for_its_issues() {
        let mut tracker = adder();
        assert_eq!(
            tracker.content(&id("1")).unwrap(),
            "pyproject.toml\n.python-version"
        );
        assert_eq!(tracker.get_status(&id("2")).unwrap(), "open");
        tracker.set_status(&id("2"), "done".to_owned()).unwrap();
        assert_eq!(tracker.get_status(&id("2")).unwrap(), "done");
        assert_eq!(
            tracker.content(&id("9")).unwrap_err().to_string(),
            "there is no issue 9"
        );
    }

    /// Each work node with the nodes directly before it.
    fn shape(dag: &Dag<String>) -> Vec<(&str, Vec<&str>)> {
        dag.nodes()
            .map(|node| {
                let before = node.predecessors().map(|before| before.value().as_str());
                (node.value().as_str(), before.collect())
            })
            .collect()
    }

    #[test]
    fn the_work_under_the_project_is_its_three_issues_in_order() {
        let dag = adder().work(&id("0")).unwrap();
        assert_eq!(
            shape(&dag),
            [("1", vec![]), ("2", vec!["1"]), ("3", vec!["1"])],
            "2 and 3 wait on the scaffolding"
        );
    }

    #[test]
    fn a_leaf_is_its_own_work() {
        assert_eq!(shape(&adder().work(&id("1")).unwrap()), [("1", vec![])]);
    }

    #[test]
    fn work_is_the_leaves_under_the_root_with_order_pushed_through_containers() {
        // The container b waits on a, so its leaf c does; x is outside.
        let tracker = JsonTracker::read(
            r#"[
                {"id": "root", "title": "", "body": "", "sub_issues": ["b", "a"]},
                {"id": "c", "title": "", "body": ""},
                {"id": "a", "title": "", "body": ""},
                {"id": "b", "title": "", "body": "", "sub_issues": ["c"], "depends_on": ["a"]},
                {"id": "x", "title": "", "body": ""}
            ]"#,
        )
        .unwrap();
        let dag = tracker.work(&id("root")).unwrap();
        assert_eq!(shape(&dag), [("a", vec![]), ("c", vec!["a"])]);
    }

    #[test]
    fn a_closed_issue_is_finished_as_a_member_and_as_a_blocker() {
        let mut tracker = JsonTracker::read(
            r#"[
                {"id": "root", "title": "", "body": "", "sub_issues": ["a", "b"]},
                {"id": "a", "title": "", "body": "", "depends_on": ["x"]},
                {"id": "b", "title": "", "body": "", "depends_on": ["a"]},
                {"id": "x", "title": "", "body": ""}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            tracker.work(&id("root")).unwrap_err().to_string(),
            "issue a is blocked by issue x, which is open and outside issue root"
        );
        tracker.set_status(&id("x"), "closed".to_owned()).unwrap();
        tracker.set_status(&id("a"), "closed".to_owned()).unwrap();
        assert_eq!(shape(&tracker.work(&id("root")).unwrap()), [("b", vec![])]);
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
        let childless = r#"[{"id": "1", "title": "", "body": "", "sub_issues": ["2"]}]"#;
        assert_eq!(
            JsonTracker::read(childless).unwrap_err().to_string(),
            "issue 1 contains 2, which is not an issue"
        );
        let cycle = r#"[
            {"id": "0", "title": "", "body": "", "sub_issues": ["1", "2"]},
            {"id": "1", "title": "", "body": "", "depends_on": ["2"]},
            {"id": "2", "title": "", "body": "", "depends_on": ["1"]}
        ]"#;
        let error = JsonTracker::read(cycle)
            .unwrap()
            .work(&id("0"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("cycle"), "{error}");
    }
}
