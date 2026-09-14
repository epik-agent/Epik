//! The GitHub verbs as tools the model can call, plus `feature_plan`.
//!
//! The verbs themselves are epik-core's table,
//! [`model_tools::github`](epik_core::model_tools::github), bound here
//! to one [`GitHub`] client. [`all`] adds `feature_plan`, the
//! [`Tracker`] seam's read of a whole feature, which needs the plan
//! vocabulary this crate holds.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{Tool, arg};
use crate::feature::{Feature, IssueId, Plan};
use crate::github::{GitHub, Repo};
use crate::tracker::Tracker;
use crate::tracker::github::GitHubTracker;

/// One tool per public GitHub verb — plus `feature_plan`, the
/// [`Tracker`] seam's read of a whole feature — all speaking through
/// `github`. Every verb takes a repository, settled against `owner`,
/// the configured default a bare name completes with.
#[must_use]
pub fn all(github: GitHub, owner: Option<String>) -> Vec<Tool> {
    let plan_client = github.clone();
    let plan_owner = owner.clone();
    let mut tools: Vec<Tool> = epik_core::model_tools::github::all(github, owner)
        .into_iter()
        .map(Tool::from)
        .collect();
    tools.push(Tool::new(
        "feature_plan",
        "A feature's whole plan in one call: the sub-issue tree rooted at the feature issue, \
         the blocked-by edges over it, any blockers outside the tree, the issues ready to \
         start, and any problems with the plan's shape. Titles and state only, no bodies. \
         Needs the GitHub token.",
        json!({
            "type": "object",
            "properties": {
                "repo": {
                    "type": "string",
                    "description": "The repository as one owner/name string, e.g. epik-agent/Epik. A bare name is accepted when a default owner is configured.",
                },
                "feature": { "type": "integer", "description": "The feature issue's number." },
            },
            "required": ["repo", "feature"],
        }),
        Box::new(move |arguments: &Value| {
            /// The whole [`Plan`] flattened into the answer — a field
            /// added to it later reaches the model unasked — plus the
            /// two readings taken of it.
            #[derive(serde::Serialize)]
            struct Shown<'a> {
                #[serde(flatten)]
                plan: &'a Plan,
                ready: Vec<&'a crate::feature::Issue>,
                problems: Vec<crate::feature::Problem>,
            }
            let repo = Repo::settle(arg::string(arguments, "repo")?, plan_owner.as_deref())?;
            let tracker = GitHubTracker::new(&plan_client, repo);
            let plan = tracker.plan(&Feature(IssueId::from(arg::number(arguments, "feature")?)))?;
            let none = BTreeSet::new();
            serde_json::to_value(Shown {
                ready: plan.ready(&none),
                problems: plan.problems(),
                plan: &plan,
            })
            .map_err(|error| error.to_string())
        }),
    ));
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> crate::tools::Registry {
        // Port 1 is reserved; a handler that reached the wire would say
        // so rather than answer.
        let mut registry = crate::tools::Registry::default();
        registry.extend(all(GitHub::at("http://127.0.0.1:1", None), None));
        registry
    }

    #[test]
    fn one_tool_per_public_verb_plus_the_trackers_plan() {
        assert_eq!(all(GitHub::at("http://127.0.0.1:1", None), None).len(), 20);
    }

    #[test]
    fn the_plan_tool_without_a_token_answers_with_the_settings_pointer() {
        let error = registry()
            .dispatch(
                "feature_plan",
                r#"{"repo":"epik-agent/Epik","feature":100}"#,
            )
            .unwrap_err();
        assert!(error.contains("Settings (Cmd+,)"), "{error}");
    }

    /// The core table's verbs answer through this registry as they do
    /// through their own.
    #[test]
    fn a_core_verb_answers_through_the_bridge() {
        let error = registry()
            .dispatch("github_issue", r#"{"repo":"not a repo","number":1}"#)
            .unwrap_err();
        assert!(error.contains("owner/name"), "{error}");
    }
}
