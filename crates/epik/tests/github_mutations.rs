//! The mutation tier: real writes, a scratch repository, never CI.
//!
//! These tests run only when two things are stated in the environment: a
//! token in `EPIK_GITHUB_TOKEN` — read through the keystore rails, the same
//! door the app uses — and a repository to scribble on in
//! `EPIK_GITHUB_SCRATCH_REPO` (`owner/name`). Absent either, they skip
//! silently: no secret enters CI, and nothing here is required anywhere.
//!
//! The scratch repository is one nobody minds filling with closed issues
//! titled "scratch". Pointing this tier at epik-agent/Epik is refused
//! outright.
//!
//! What is asserted is the round trip: an edge added is an edge the graph
//! then shows, and removed is gone; an issue closed reads back closed; a
//! pull opened from a scaffolded branch merges and reads back merged. The
//! branch scaffolding itself uses the raw git data API here in the test,
//! because pushing commits is not one of Epik's verbs.

// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use epik::github::{Error, GitHub, Merge, Repo, State};
use epik::keystore::{InMemory, Keys, Resolved};
use serde_json::{Value, json};

const SCRATCH_ENV: &str = "EPIK_GITHUB_SCRATCH_REPO";

/// The client and the repository to scribble on, or `None` when this tier is
/// skipping — silently, because it is required nowhere.
fn scratch() -> Option<(GitHub, Repo, String)> {
    let Resolved::Found(token) = Keys::new(InMemory::default()).github_token() else {
        return None;
    };
    let named = env::var(SCRATCH_ENV).ok()?;
    let (owner, name) = named
        .split_once('/')
        .unwrap_or_else(|| panic!("{SCRATCH_ENV} should be owner/name, not {named:?}"));
    let repo = Repo::new(owner, name);
    assert!(
        !(repo.owner == "epik-agent" && repo.name == "Epik"),
        "the mutation tier never runs against epik-agent/Epik itself"
    );
    Some((GitHub::new(Some(token.clone())), repo, token))
}

/// Something no two runs share, so scratch issues and branches never collide.
fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[test]
fn issues_and_their_edges_round_trip() {
    let Some((github, repo, _)) = scratch() else {
        return;
    };
    let stamp = stamp();

    // Create, edit, comment.
    let parent = github
        .create_issue(&repo, &format!("scratch parent {stamp}"), "born in a test")
        .unwrap();
    let child = github
        .create_issue(&repo, &format!("scratch child {stamp}"), "born in a test")
        .unwrap();
    assert_eq!(parent.state, State::Open);

    let edited = github
        .edit_issue(
            &repo,
            parent.number,
            Some(&format!("scratch parent {stamp} (edited)")),
            Some("edited in a test"),
        )
        .unwrap();
    assert!(edited.title.ends_with("(edited)"));
    assert_eq!(edited.body, "edited in a test");

    github
        .comment(&repo, parent.number, "a comment from the mutation tier")
        .unwrap();

    // A sub-issue edge appears in the graph, then disappears.
    github
        .add_sub_issue(&repo, parent.number, child.number)
        .unwrap();
    let graph = github.issue_graph(&repo, parent.number).unwrap();
    assert!(
        graph
            .sub_issues
            .iter()
            .any(|edge| edge.number == child.number),
        "{graph:?}"
    );

    github
        .remove_sub_issue(&repo, parent.number, child.number)
        .unwrap();
    let graph = github.issue_graph(&repo, parent.number).unwrap();
    assert!(
        !graph
            .sub_issues
            .iter()
            .any(|edge| edge.number == child.number),
        "{graph:?}"
    );

    // A blocked-by edge, likewise.
    github
        .add_blocked_by(&repo, parent.number, child.number)
        .unwrap();
    let graph = github.issue_graph(&repo, parent.number).unwrap();
    assert!(
        graph
            .blocked_by
            .iter()
            .any(|edge| edge.number == child.number),
        "{graph:?}"
    );

    github
        .remove_blocked_by(&repo, parent.number, child.number)
        .unwrap();
    let graph = github.issue_graph(&repo, parent.number).unwrap();
    assert!(graph.blocked_by.is_empty(), "{graph:?}");

    // Close both; closed is what reads back.
    assert_eq!(
        github.close_issue(&repo, child.number).unwrap().state,
        State::Closed
    );
    assert_eq!(
        github.close_issue(&repo, parent.number).unwrap().state,
        State::Closed
    );
}

#[test]
fn a_pull_opens_and_merges() {
    let Some((github, repo, token)) = scratch() else {
        return;
    };
    let branch = format!("epik-scratch-{}", stamp());

    let base = github.default_branch(&repo).unwrap();
    scaffold_branch(&repo, &token, &base, &branch);

    let opened = github
        .open_pull(
            &repo,
            &format!("scratch: {branch}"),
            &branch,
            &base,
            "opened by the mutation tier",
        )
        .unwrap();
    assert_eq!(opened.state, State::Open);
    assert!(!opened.merged);
    assert_eq!(opened.head.name, branch);
    assert_eq!(opened.base.name, base);

    github
        .merge_pull(&repo, opened.number, Merge::Squash)
        .unwrap();

    let merged = github.pull(&repo, opened.number).unwrap();
    assert_eq!(merged.state, State::Closed);
    assert!(merged.merged);

    // Merging a pull request that does not exist is a refusal in GitHub's
    // own words, not a panic. (Merging twice, it turns out, is a success:
    // GitHub answers a re-merge of a merged PR with 200.)
    let error = github
        .merge_pull(&repo, 999_999_999, Merge::Squash)
        .unwrap_err();
    assert!(matches!(error, Error::Refused { .. }), "{error:?}");

    // Best-effort cleanup; a leftover branch in a scratch repo hurts nobody.
    let _ = raw(
        &token,
        "DELETE",
        &format!("repos/{repo}/git/refs/heads/{branch}"),
        None,
    );
}

/// Grows `branch` off `base` with one commit, via the raw git data API.
/// Test scaffolding, not a library verb: Epik pushes with git, not HTTP.
fn scaffold_branch(repo: &Repo, token: &str, base: &str, branch: &str) {
    let head = raw(
        token,
        "GET",
        &format!("repos/{repo}/git/ref/heads/{base}"),
        None,
    )
    .unwrap();
    let head_sha = head["object"]["sha"].as_str().unwrap().to_owned();

    let blob = raw(
        token,
        "POST",
        &format!("repos/{repo}/git/blobs"),
        Some(json!({ "content": format!("scratch {branch}\n"), "encoding": "utf-8" })),
    )
    .unwrap();

    let commit = raw(
        token,
        "GET",
        &format!("repos/{repo}/git/commits/{head_sha}"),
        None,
    )
    .unwrap();

    let tree = raw(
        token,
        "POST",
        &format!("repos/{repo}/git/trees"),
        Some(json!({
            "base_tree": commit["tree"]["sha"],
            "tree": [{ "path": format!("{branch}.txt"), "mode": "100644",
                       "type": "blob", "sha": blob["sha"] }],
        })),
    )
    .unwrap();

    let new_commit = raw(
        token,
        "POST",
        &format!("repos/{repo}/git/commits"),
        Some(json!({
            "message": format!("scratch commit for {branch}"),
            "tree": tree["sha"],
            "parents": [head_sha],
        })),
    )
    .unwrap();

    raw(
        token,
        "POST",
        &format!("repos/{repo}/git/refs"),
        Some(json!({ "ref": format!("refs/heads/{branch}"), "sha": new_commit["sha"] })),
    )
    .unwrap();
}

/// One raw REST exchange against api.github.com, for scaffolding only.
fn raw(token: &str, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let agent = ureq::Agent::new_with_defaults();
    let url = format!("https://api.github.com/{path}");
    let authorization = format!("Bearer {token}");
    let response = match (method, body) {
        ("GET", None) => agent
            .get(&url)
            .header("authorization", &authorization)
            .call(),
        ("POST", Some(body)) => agent
            .post(&url)
            .header("authorization", &authorization)
            .send_json(&body),
        ("DELETE", None) => agent
            .delete(&url)
            .header("authorization", &authorization)
            .call(),
        _ => panic!("unscripted scaffolding request: {method} {path}"),
    };
    let mut response = response.map_err(|error| format!("{method} {url}: {error}"))?;
    let body = response.body_mut().read_to_string().unwrap_or_default();
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).map_err(|error| format!("{method} {url}: {error}: {body}"))
}
