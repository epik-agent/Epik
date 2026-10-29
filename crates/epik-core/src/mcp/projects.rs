//! `project_list`, `project_list_items`, `project_get_item` and
//! `project_set_status`: GitHub Projects, which only GraphQL reaches.

//!
//! Every call looks up the IDs it needs afresh: one extra query is cheaper
//! than a cache that goes stale. Users and organizations are both
//! `ProjectV2Owner`s, so one query serves both.

use serde_json::{Map, Value, json};

use super::epic_status::issue_url;
use super::gh::Gh;
use super::{Args, Context, Reply, integer, string};

/// Selects `selection` on the `ProjectV2Owner` `owner` and returns it.
/// `variables` may hold a `number` and a `cursor`.
fn project_owner(
    gh: &Gh,
    owner: &str,
    selection: &str,
    variables: &[(&str, Value)],
) -> Result<Value, String> {
    let declarations: String = variables
        .iter()
        .map(|(name, _)| {
            let kind = if *name == "number" { "Int!" } else { "String" };
            format!(", ${name}: {kind}")
        })
        .collect();
    let query = format!(
        "query($owner: String!{declarations}) {{
      repositoryOwner(login: $owner) {{ ... on ProjectV2Owner {{ {selection} }} }}
    }}"
    );
    let mut all = vec![("owner", json!(owner))];
    all.extend(variables.iter().cloned());
    let data = gh.graphql(&query, &all)?;
    match &data["repositoryOwner"] {
        Value::Null => Err(format!(
            "There is no GitHub user or organization named {owner}."
        )),
        found => Ok(found.clone()),
    }
}

/// Selects `selection` on project `owner` #`number` and returns it.
/// `variables` may hold a `cursor`.
fn project(
    gh: &Gh,
    owner: &str,
    number: i64,
    selection: &str,
    variables: &[(&str, Value)],
) -> Result<Value, String> {
    let mut all = vec![("number", json!(number))];
    all.extend(variables.iter().cloned());
    let found = project_owner(
        gh,
        owner,
        &format!("projectV2(number: $number) {{ {selection} }}"),
        &all,
    )?;
    match &found["projectV2"] {
        Value::Null => Err(format!(
            "{owner} has no project #{number}, or it is not visible to you."
        )),
        project => Ok(project.clone()),
    }
}

/// Every node of a paginated connection. `page` reads the connection at a
/// cursor.
fn paginate(
    mut page: impl FnMut(Option<&str>) -> Result<Value, String>,
) -> Result<Vec<Value>, String> {
    let mut nodes = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let connection = page(cursor.as_deref())?;
        if let Some(page) = connection["nodes"].as_array() {
            nodes.extend(page.iter().cloned());
        }
        let info = &connection["pageInfo"];
        cursor = match (info["hasNextPage"].as_bool(), info["endCursor"].as_str()) {
            (Some(true), Some(end)) => Some(end.to_owned()),
            _ => None,
        };
        if cursor.is_none() {
            return Ok(nodes);
        }
    }
}

/// Selects an item's field values, each as `value`, for [`field_values`].
fn field_values_selection() -> String {
    let fields = [
        ("SingleSelect", "name"),
        ("Text", "text"),
        ("Number", "number"),
        ("Date", "date"),
        ("Iteration", "title"),
    ]
    .iter()
    .map(|(kind, value)| {
        format!(
            "... on ProjectV2ItemField{kind}Value {{ value: {value} field {{ ... on ProjectV2FieldCommon {{ name }} }} }}"
        )
    })
    .collect::<Vec<_>>()
    .join("\n    ");
    format!("fieldValues(first: 50) {{\n  nodes {{\n    {fields}\n  }}\n}}")
}

/// An item's field values as `{ field name: value }`.
fn field_values(item: &Value) -> Map<String, Value> {
    item["fieldValues"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let name = node["field"]["name"].as_str()?;
            Some((name.to_owned(), node["value"].clone()))
        })
        .collect()
}

/// The issue a tool's arguments name, and the project it may be in.
struct IssueInProject<'a> {
    owner: &'a str,
    number: i64,
    repo: &'a str,
    issue_number: i64,
}

impl<'a> IssueInProject<'a> {
    fn of(args: &'a Args) -> Self {
        Self {
            owner: string(args, "owner").unwrap_or_default(),
            number: integer(args, "number").unwrap_or_default(),
            repo: string(args, "repo").unwrap_or_default(),
            issue_number: integer(args, "issue_number").unwrap_or_default(),
        }
    }

    /// The issue's node ID and its item in the project, or `None` when the
    /// issue is not in that project.
    fn item(&self, gh: &Gh) -> Result<(Value, Option<Value>), String> {
        let (repo_owner, name) = self.repo.split_once('/').unwrap_or((self.repo, ""));
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
        repository(owner: $owner, name: $name) {{
          issue(number: $number) {{
            id
            projectItems(first: 100) {{
              nodes {{
                id
                project {{ number owner {{ ... on User {{ login }} ... on Organization {{ login }} }} }}
                {}
              }}
            }}
          }}
        }}
      }}",
            field_values_selection()
        );
        let data = gh.graphql(
            &query,
            &[
                ("owner", json!(repo_owner)),
                ("name", json!(name)),
                ("number", json!(self.issue_number)),
            ],
        )?;
        let issue = &data["repository"]["issue"];
        if issue.is_null() {
            return Err(format!(
                "There is no issue #{} in {}.",
                self.issue_number, self.repo
            ));
        }
        let item = issue["projectItems"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| {
                let project = &item["project"];
                project["number"].as_i64() == Some(self.number)
                    && project["owner"]["login"]
                        .as_str()
                        .is_some_and(|login| login.eq_ignore_ascii_case(self.owner))
            })
            .cloned();
        Ok((issue["id"].clone(), item))
    }
}

pub fn list(context: &Context, args: &Args) -> Result<Reply, String> {
    let owner = string(args, "owner").unwrap_or_default();
    let projects = paginate(|cursor| {
        let found = project_owner(
            &context.gh,
            owner,
            "projectsV2(first: 100, after: $cursor) {
            pageInfo { hasNextPage endCursor }
            nodes { number title url closed }
          }",
            &[("cursor", json!(cursor))],
        )?;
        Ok(found["projectsV2"].clone())
    })?;
    Ok(Reply::Json(
        projects
            .iter()
            .filter(|project| project["closed"] != true)
            .map(|project| {
                json!({"number": project["number"], "title": project["title"], "url": project["url"]})
            })
            .collect(),
    ))
}

pub fn list_items(context: &Context, args: &Args) -> Result<Reply, String> {
    let owner = string(args, "owner").unwrap_or_default();
    let number = integer(args, "number").unwrap_or_default();
    let status_filter = string(args, "status_filter");
    let selection = format!(
        "items(first: 100, after: $cursor) {{
            pageInfo {{ hasNextPage endCursor }}
            nodes {{
              content {{ ... on Issue {{ number title repository {{ nameWithOwner }} }} }}
              {}
            }}
          }}",
        field_values_selection()
    );
    let items = paginate(|cursor| {
        let variables = [("cursor", json!(cursor))];
        Ok(project(&context.gh, owner, number, &selection, &variables)?["items"].clone())
    })?;
    Ok(Reply::Json(
        items
            .iter()
            .filter_map(|item| {
                let content = &item["content"];
                let issue_number = content["number"].as_u64().filter(|n| *n != 0)?;
                let repo = content["repository"]["nameWithOwner"]
                    .as_str()
                    .unwrap_or_default();
                let status = field_values(item).remove("Status").unwrap_or(Value::Null);
                Some(json!({
                    "issue_number": issue_number,
                    "title": content["title"],
                    "url": issue_url(repo, issue_number),
                    "repo": repo,
                    "status": status,
                }))
            })
            .filter(|item| status_filter.is_none_or(|filter| item["status"] == filter))
            .collect(),
    ))
}

pub fn get_item(context: &Context, args: &Args) -> Result<Reply, String> {
    let (_, item) = IssueInProject::of(args).item(&context.gh)?;
    Ok(Reply::Json(match item {
        Some(item) => json!({
            "found": true,
            "item_id": item["id"],
            "fields": field_values(&item),
        }),
        None => json!({"found": false}),
    }))
}

pub fn set_status(context: &Context, args: &Args) -> Result<Reply, String> {
    let issue = IssueInProject::of(args);
    let status = string(args, "status_name").unwrap_or_default();
    let (owner, number) = (issue.owner, issue.number);
    let found = project(
        &context.gh,
        owner,
        number,
        "id
    field(name: \"Status\") { ... on ProjectV2SingleSelectField { id options { id name } } }",
        &[],
    )?;
    let field = &found["field"];
    if field["id"].is_null() {
        return Err(format!("Project {owner} #{number} has no Status field."));
    }
    let (issue_id, item) = issue.item(&context.gh)?;
    let options = field["options"].as_array().cloned().unwrap_or_default();
    let Some(option) = options.iter().find(|option| option["name"] == status) else {
        let statuses: Vec<&str> = options
            .iter()
            .filter_map(|option| option["name"].as_str())
            .collect();
        return Err(format!(
            "Project {owner} #{number} has no status \"{status}\". Its statuses are {}.",
            statuses.join(", ")
        ));
    };
    let project_id = &found["id"];
    let item_id = match item {
        Some(item) => item["id"].clone(),
        None => {
            let data = context.gh.graphql(
                "mutation($project: ID!, $content: ID!) {
      addProjectV2ItemById(input: { projectId: $project, contentId: $content }) { item { id } }
    }",
                &[("project", project_id.clone()), ("content", issue_id)],
            )?;
            data["addProjectV2ItemById"]["item"]["id"].clone()
        }
    };
    context.gh.graphql(
        "mutation($project: ID!, $item: ID!, $field: ID!, $option: String!) {
        updateProjectV2ItemFieldValue(input: {
          projectId: $project, itemId: $item, fieldId: $field, value: { singleSelectOptionId: $option }
        }) { projectV2Item { id } }
      }",
        &[
            ("project", project_id.clone()),
            ("item", item_id.clone()),
            ("field", field["id"].clone()),
            ("option", option["id"].clone()),
        ],
    )?;
    Ok(Reply::Json(json!({"item_id": item_id, "status": status})))
}

#[cfg(test)]
mod tests {
    use crate::mcp::tests::{Calls, call_json, scripted};
    use serde_json::{Value, json};

    /// The call whose arguments, joined by spaces, contain `needle`.
    fn call(calls: &Calls, needle: &str) -> Option<Vec<String>> {
        calls
            .lock()
            .unwrap()
            .iter()
            .find(|args| args.join(" ").contains(needle))
            .cloned()
    }

    fn tail(args: &[String], n: usize) -> Vec<&str> {
        args[args.len() - n..].iter().map(String::as_str).collect()
    }

    fn owned(selection: Value) -> Value {
        json!({"data": {"repositoryOwner": selection}})
    }

    fn page(nodes: Value, end_cursor: Option<&str>) -> Value {
        json!({"pageInfo": {"hasNextPage": end_cursor.is_some(), "endCursor": end_cursor}, "nodes": nodes})
    }

    fn item(number: u64, status: &str) -> Value {
        json!({
            "content": {"number": number, "title": format!("Issue {number}"), "repository": {"nameWithOwner": "epik-agent/Epik"}},
            "fieldValues": {"nodes": [{"value": status, "field": {"name": "Status"}}, {}]},
        })
    }

    fn status_field() -> (&'static str, Value) {
        (
            "field(name: \"Status\")",
            owned(json!({
                "projectV2": {
                    "id": "project-12",
                    "field": {"id": "field-status", "options": [
                        {"id": "opt-todo", "name": "Todo"},
                        {"id": "opt-progress", "name": "In Progress"},
                        {"id": "opt-done", "name": "Done"},
                    ]},
                },
            })),
        )
    }

    fn issue_311() -> Value {
        json!({"owner": "epik-agent", "number": 12, "repo": "epik-agent/Epik", "issue_number": 311})
    }

    fn with_status(status: &str) -> Value {
        let mut args = issue_311();
        args["status_name"] = json!(status);
        args
    }

    fn project(number: u64, login: &str) -> Value {
        json!({"number": number, "owner": {"login": login}})
    }

    fn issue_items(nodes: Value) -> (&'static str, Value) {
        (
            "projectItems",
            json!({"data": {"repository": {"issue": {"id": "issue-311", "projectItems": {"nodes": nodes}}}}}),
        )
    }

    fn in_project_12() -> (&'static str, Value) {
        issue_items(
            json!([{"id": "item-311", "project": project(12, "Epik-Agent"), "fieldValues": {"nodes": []}}]),
        )
    }

    fn updated() -> (&'static str, Value) {
        (
            "updateProjectV2ItemFieldValue",
            json!({"data": {"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": "item-311"}}}}),
        )
    }

    #[test]
    fn user_and_organization_owners_are_both_queried_as_a_project_v2_owner() {
        let (gh, calls) = scripted(vec![(
            "projectsV2",
            owned(json!({"projectsV2": page(json!([]), None)})),
        )]);
        let server = crate::mcp::tests::server_with(gh);

        for owner in ["epik-agent", "wpm"] {
            let result = server
                .call("project_list", json!({"owner": owner}))
                .unwrap();
            assert!(result.is_error != Some(true));
        }

        let calls = calls.lock().unwrap();
        let (organization, user) = (&calls[0][3..], &calls[1][3..]);
        let pattern = regex_lite::Regex::new(
            r"repositoryOwner\(login: \$owner\) \{ \.\.\. on ProjectV2Owner",
        )
        .unwrap();
        assert!(pattern.is_match(&organization[0]), "{}", organization[0]);
        // The first page has no cursor, but the query still declares it.
        assert!(
            organization[0].contains("query($owner: String!, $cursor: String)"),
            "{}",
            organization[0]
        );
        assert_eq!(organization[1..], ["-f", "owner=epik-agent"]);
        assert_eq!(user, [organization[0].as_str(), "-f", "owner=wpm"]);
    }

    #[test]
    fn an_owner_that_does_not_exist_is_named_in_the_error() {
        let (gh, _) = scripted(vec![("projectsV2", owned(Value::Null))]);

        let error = call_json(gh, "project_list", json!({"owner": "nobody"})).unwrap_err();

        assert_eq!(
            error,
            "There is no GitHub user or organization named nobody."
        );
    }

    #[test]
    fn project_list_returns_the_open_projects_across_every_page() {
        let listed = |number: u64, closed: bool| json!({"number": number, "title": format!("P{number}"), "url": format!("https://github.com/orgs/epik-agent/projects/{number}"), "closed": closed});
        let (gh, _) = scripted(vec![
            (
                "cursor=page-2",
                owned(json!({"projectsV2": page(json!([listed(12, false)]), None)})),
            ),
            (
                "projectsV2",
                owned(
                    json!({"projectsV2": page(json!([listed(3, true), listed(7, false)]), Some("page-2"))}),
                ),
            ),
        ]);

        let projects = call_json(gh, "project_list", json!({"owner": "epik-agent"}));

        assert_eq!(
            projects.unwrap(),
            json!([
                {"number": 7, "title": "P7", "url": "https://github.com/orgs/epik-agent/projects/7"},
                {"number": 12, "title": "P12", "url": "https://github.com/orgs/epik-agent/projects/12"},
            ])
        );
    }

    #[test]
    fn project_list_items_returns_each_issue_with_its_status_filtered_by_status() {
        let (gh, calls) = scripted(vec![(
            "items(",
            owned(json!({"projectV2": {"items": page(
                json!([item(1, "Done"), {"content": {}, "fieldValues": {"nodes": []}}, item(2, "Todo")]),
                None,
            )}})),
        )]);

        let items = call_json(
            gh,
            "project_list_items",
            json!({"owner": "epik-agent", "number": 12, "status_filter": "Todo"}),
        );

        assert_eq!(
            items.unwrap(),
            json!([{
                "issue_number": 2,
                "title": "Issue 2",
                "url": "https://github.com/epik-agent/epik/issues/2",
                "repo": "epik-agent/Epik",
                "status": "Todo",
            }])
        );
        assert_eq!(
            tail(&call(&calls, "items(").unwrap(), 4),
            ["-f", "owner=epik-agent", "-F", "number=12"]
        );
    }

    #[test]
    fn project_list_items_without_a_filter_returns_every_issue() {
        let (gh, _) = scripted(vec![(
            "items(",
            owned(json!({"projectV2": {"items": page(
                json!([item(1, "Done"), {"content": {"number": 3, "title": "No status", "repository": {"nameWithOwner": "o/r"}}, "fieldValues": {"nodes": []}}]),
                None,
            )}})),
        )]);

        let items = call_json(
            gh,
            "project_list_items",
            json!({"owner": "epik-agent", "number": 12}),
        )
        .unwrap();

        assert_eq!(items[0]["status"], "Done");
        assert_eq!(items[1]["status"], Value::Null);
        assert_eq!(items.as_array().unwrap().len(), 2);
    }

    #[test]
    fn project_get_item_returns_the_items_field_values() {
        let (gh, calls) = scripted(vec![issue_items(json!([
            {"id": "other-owner", "project": project(12, "wpm"), "fieldValues": {"nodes": []}},
            {"id": "item-311", "project": project(12, "epik-agent"), "fieldValues": {"nodes": [
                {"value": "In Progress", "field": {"name": "Status"}},
                {"value": "The Node server reads and sets GitHub Project status", "field": {"name": "Title"}},
                {"value": 3, "field": {"name": "Estimate"}},
                {},
            ]}},
        ]))]);

        let found = call_json(gh, "project_get_item", issue_311());

        assert_eq!(
            found.unwrap(),
            json!({
                "found": true,
                "item_id": "item-311",
                "fields": {"Status": "In Progress", "Title": "The Node server reads and sets GitHub Project status", "Estimate": 3},
            })
        );
        assert_eq!(
            tail(&call(&calls, "projectItems").unwrap(), 6),
            [
                "-f",
                "owner=epik-agent",
                "-f",
                "name=Epik",
                "-F",
                "number=311"
            ]
        );
    }

    #[test]
    fn project_get_item_reports_an_issue_outside_the_project_as_not_found() {
        let (gh, _) = scripted(vec![issue_items(
            json!([{"id": "item-7", "project": project(7, "epik-agent"), "fieldValues": {"nodes": []}}]),
        )]);

        let found = call_json(gh, "project_get_item", issue_311());

        assert_eq!(found.unwrap(), json!({"found": false}));
    }

    #[test]
    fn project_set_status_sets_the_status_option_on_the_existing_item() {
        let (gh, calls) = scripted(vec![updated(), status_field(), in_project_12()]);

        let set = call_json(gh, "project_set_status", with_status("Done"));

        assert_eq!(
            set.unwrap(),
            json!({"item_id": "item-311", "status": "Done"})
        );
        assert_eq!(call(&calls, "addProjectV2ItemById"), None);
        assert_eq!(
            tail(&call(&calls, "updateProjectV2ItemFieldValue").unwrap(), 8),
            [
                "-f",
                "project=project-12",
                "-f",
                "item=item-311",
                "-f",
                "field=field-status",
                "-f",
                "option=opt-done"
            ]
        );
    }

    #[test]
    fn project_set_status_adds_an_issue_that_is_not_yet_in_the_project() {
        let (gh, calls) = scripted(vec![
            updated(),
            (
                "addProjectV2ItemById",
                json!({"data": {"addProjectV2ItemById": {"item": {"id": "item-new"}}}}),
            ),
            status_field(),
            issue_items(json!([])),
        ]);

        let set = call_json(gh, "project_set_status", with_status("Todo"));

        assert_eq!(
            set.unwrap(),
            json!({"item_id": "item-new", "status": "Todo"})
        );
        assert_eq!(
            tail(&call(&calls, "addProjectV2ItemById").unwrap(), 4),
            ["-f", "project=project-12", "-f", "content=issue-311"]
        );
        let update = call(&calls, "updateProjectV2ItemFieldValue").unwrap();
        assert_eq!(
            update[update.len() - 6..update.len() - 4],
            ["-f", "item=item-new"]
        );
    }

    #[test]
    fn an_unknown_status_names_the_available_ones() {
        let (gh, _) = scripted(vec![status_field(), in_project_12()]);

        let error = call_json(gh, "project_set_status", with_status("Blocked")).unwrap_err();

        assert_eq!(
            error,
            "Project epik-agent #12 has no status \"Blocked\". Its statuses are Todo, In Progress, Done."
        );
    }

    #[test]
    fn a_project_without_a_status_field_says_so() {
        let (gh, _) = scripted(vec![
            (
                "field(name: \"Status\")",
                owned(json!({"projectV2": {"id": "project-12", "field": null}})),
            ),
            in_project_12(),
        ]);

        let error = call_json(gh, "project_set_status", with_status("Done")).unwrap_err();

        assert_eq!(error, "Project epik-agent #12 has no Status field.");
    }

    #[test]
    fn a_project_that_does_not_exist_is_named_in_the_error() {
        let replies = || {
            scripted(vec![
                ("projectV2(number", owned(json!({"projectV2": null}))),
                in_project_12(),
            ])
            .0
        };
        let missing = "epik-agent has no project #999, or it is not visible to you.";

        let listed = call_json(
            replies(),
            "project_list_items",
            json!({"owner": "epik-agent", "number": 999}),
        );
        let mut args = with_status("Done");
        args["number"] = json!(999);
        let set = call_json(replies(), "project_set_status", args);

        assert_eq!(listed.unwrap_err(), missing);
        assert_eq!(set.unwrap_err(), missing);
    }
}
