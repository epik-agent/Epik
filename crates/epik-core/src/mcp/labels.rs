//! `label_list`, `label_create` and `label_delete`: a repository's labels,
//! through `gh label`.

use serde_json::json;

use super::gh::strings;
use super::{Args, Context, Reply, boolean, nonempty, string};

pub fn list(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let labels = context.gh.json(&strings([
        "label",
        "list",
        "--repo",
        repo,
        "--json",
        "name,color,description",
    ]))?;
    Ok(Reply::Json(labels))
}

pub fn create(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let name = string(args, "name").unwrap_or_default();
    let color = string(args, "color").unwrap_or_default();
    let description = nonempty(args, "description");
    let mut command = strings(["label", "create", name, "--repo", repo, "--color", color]);
    if let Some(description) = description {
        command.extend(strings(["--description", description]));
    }
    if boolean(args, "force") {
        command.push("--force".to_owned());
    }
    context.gh.text(&command)?;
    Ok(Reply::Json(json!({
        "name": name,
        "color": color,
        "description": description.unwrap_or_default(),
    })))
}

pub fn delete(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let name = string(args, "name").unwrap_or_default();
    context
        .gh
        .text(&strings(["label", "delete", name, "--repo", repo, "--yes"]))?;
    Ok(Reply::Json(json!({"deleted": name, "repo": repo})))
}

#[cfg(test)]
mod tests {
    use crate::mcp::tests::{call_json, expect_gh, failing_gh};
    use serde_json::{Value, json};

    fn epic() -> Value {
        json!({"name": "epic", "color": "5319e7", "description": "An issue with sub-issues"})
    }

    #[test]
    fn label_list_returns_every_label_with_its_color_and_description() {
        let gh = expect_gh(
            "label list --repo o/r --json name,color,description",
            &json!([epic()]).to_string(),
        );

        let labels = call_json(gh, "label_list", json!({"repo": "o/r"}));

        assert_eq!(labels.unwrap(), json!([epic()]));
    }

    #[test]
    fn label_list_fails_when_there_is_no_such_repository() {
        let error = call_json(
            failing_gh("HTTP 404: Not Found"),
            "label_list",
            json!({"repo": "o/nope"}),
        )
        .unwrap_err();

        assert!(error.contains("404"), "{error}");
    }

    #[test]
    fn label_create_creates_a_label_and_returns_it() {
        let gh = expect_gh("label create epic --repo o/r --color 5319e7", "");

        let label = call_json(
            gh,
            "label_create",
            json!({"repo": "o/r", "name": "epic", "color": "5319e7"}),
        );

        assert_eq!(
            label.unwrap(),
            json!({"name": "epic", "color": "5319e7", "description": ""})
        );
    }

    #[test]
    fn label_create_with_force_updates_an_existing_label() {
        let gh = expect_gh(
            "label create epic --repo o/r --color 5319e7 --description An issue with sub-issues --force",
            "",
        );
        let mut args = epic();
        args["repo"] = json!("o/r");
        args["force"] = json!(true);

        let label = call_json(gh, "label_create", args);

        assert_eq!(label.unwrap(), epic());
    }

    #[test]
    fn label_create_fails_when_the_label_exists() {
        let gh = failing_gh("label with name \"epic\" already exists; use --force to update");

        let error = call_json(
            gh,
            "label_create",
            json!({"repo": "o/r", "name": "epic", "color": "5319e7"}),
        )
        .unwrap_err();

        assert!(error.contains("already exists"), "{error}");
    }

    #[test]
    fn label_delete_deletes_the_label() {
        let gh = expect_gh("label delete epic --repo o/r --yes", "");

        let deleted = call_json(gh, "label_delete", json!({"repo": "o/r", "name": "epic"}));

        assert_eq!(deleted.unwrap(), json!({"deleted": "epic", "repo": "o/r"}));
    }

    #[test]
    fn label_delete_fails_when_there_is_no_such_label() {
        let error = call_json(
            failing_gh("HTTP 404: Not Found"),
            "label_delete",
            json!({"repo": "o/r", "name": "nope"}),
        )
        .unwrap_err();

        assert!(error.contains("404"), "{error}");
    }
}
