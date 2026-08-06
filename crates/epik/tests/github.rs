//! The GitHub smoke: the real API, no token, this very repository.
//!
//! Everything here reads epik-agent/Epik unauthenticated, which is exactly
//! what a fresh install can do and exactly what secretless CI can do. On a
//! laptop with no network the tier skips — the same rule as the live model
//! tier — and `EPIK_REQUIRE_GITHUB=1` turns that skip into a failure, so in
//! CI the tier cannot look green forever without ever having executed.
//!
//! Rate limiting skips even when the tier is required, deliberately: CI
//! runners share addresses, unauthenticated reads share a small budget, and
//! a classified [`Error::RateLimited`] is itself evidence that the client
//! reached GitHub, read the refusal, and told it apart from failure — which
//! is most of what a smoke test is for.
//!
//! What is asserted is durable: the default branch's name, a merged PR's
//! anatomy, that checks concluded on `main`. Nothing here asserts the state
//! of an issue that someone might close tomorrow.

// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::env;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use epik::github::{Error, GitHub, Repo, State};

/// Set this and an unreachable GitHub stops being a reason to skip.
const REQUIRE_ENV: &str = "EPIK_REQUIRE_GITHUB";

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn epik() -> Repo {
    Repo::new("epik-agent", "Epik")
}

/// The client to smoke with, or `None` when this tier is skipping.
///
/// # Panics
///
/// Panics under `EPIK_REQUIRE_GITHUB=1` when GitHub is unreachable.
fn smoke() -> Option<GitHub> {
    if listening("api.github.com:443") {
        return Some(GitHub::new(None));
    }
    let required = env::var(REQUIRE_ENV).unwrap_or_default() == "1";
    assert!(
        !required,
        "{REQUIRE_ENV}=1, but api.github.com is unreachable from here"
    );
    eprintln!(
        "skipping the GitHub smoke: api.github.com is unreachable. \
         Set {REQUIRE_ENV}=1 to make this a failure."
    );
    None
}

/// Whether anything answers at `address`. "Is the network there" is the
/// actual question, so ask TCP rather than HTTP.
fn listening(address: &str) -> bool {
    address
        .to_socket_addrs()
        .into_iter()
        .flatten()
        .any(|address| TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok())
}

/// Unwraps a read, treating a rate limit as a skip rather than a failure —
/// the reason [`Error::RateLimited`] is its own variant.
fn tolerating<T>(outcome: Result<T, Error>) -> Option<T> {
    match outcome {
        Ok(value) => Some(value),
        Err(Error::RateLimited(message)) => {
            eprintln!(
                "skipping: GitHub rate-limited this address — which the \
                 client classified, so the wire works: {message}"
            );
            None
        }
        Err(error) => panic!("the smoke read failed: {error}"),
    }
}

#[test]
fn the_default_branch_reads_without_a_token() {
    let Some(github) = smoke() else { return };
    let Some(branch) = tolerating(github.default_branch(&epik())) else {
        return;
    };
    assert_eq!(branch, "main");
}

#[test]
fn an_issue_reads_without_a_token() {
    let Some(github) = smoke() else { return };
    // The issue that specified this module. Its title is quoted in the
    // design history, so asserting a word of it is not a tautology.
    let Some(issue) = tolerating(github.issue(&epik(), 106)) else {
        return;
    };
    assert_eq!(issue.number, 106);
    assert!(issue.title.contains("github.rs"), "{}", issue.title);
    assert!(!issue.body.is_empty());
}

#[test]
fn a_merged_pull_reads_with_its_whole_anatomy() {
    let Some(github) = smoke() else { return };
    // PR #91 merged the chatbot walking skeleton; merged PRs do not unmerge.
    let Some(pull) = tolerating(github.pull(&epik(), 91)) else {
        return;
    };
    assert_eq!(pull.number, 91);
    assert_eq!(pull.state, State::Closed);
    assert!(pull.merged);
    assert_eq!(pull.head.name, "local-client-greenfield");
    assert_eq!(pull.base.name, "main");
    assert_eq!(pull.head.sha.len(), 40, "{}", pull.head.sha);
}

#[test]
fn checks_concluded_on_the_default_branch() {
    let Some(github) = smoke() else { return };
    let repo = epik();
    let Some(branch) = tolerating(github.default_branch(&repo)) else {
        return;
    };
    let Some(checks) = tolerating(github.check_conclusions(&repo, &branch)) else {
        return;
    };
    // Nothing lands on main without CI, so its head always has check runs.
    assert!(!checks.is_empty(), "no check runs on {branch}");
    assert!(
        checks.iter().all(|check| !check.name.is_empty()),
        "{checks:?}"
    );
}
