//! Talking to GitHub.
//!
//! One hand-rolled client, on the [`OpenAiCompatible`] precedent: the
//! protocol is spoken directly, the surface covers exactly Epik's verbs, and
//! provider quirks are ours to absorb rather than a dependency's to
//! reinterpret. REST carries the ordinary verbs — issues, pull requests,
//! check conclusions, the default branch — and GraphQL (a POST to
//! `/graphql`) carries the relationship mutations that never got REST
//! endpoints: sub-issue and blocked-by edges. Query strings and JSON shapes
//! stay inside this module; callers see types.
//!
//! The token arrives on the [`keystore`](crate::keystore) rails and is
//! optional on purpose: reading a public repository needs none, so only the
//! verbs that write — and GraphQL, which answers nothing unauthenticated —
//! refuse without one. The refusal is [`Error::TokenAbsent`], a value to
//! render, never a panic or a prose log line. Rate limiting gets its own
//! variant for the same reason: "come back later" and "no" call for
//! different behavior, so they are different values.
//!
//! Everything here is blocking, on purpose, like the rest of the library.
//! Async is the host's problem.
//!
//! [`OpenAiCompatible`]: crate::chat::OpenAiCompatible

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub mod tools;

/// Where GitHub is. One value, because Epik talks to one GitHub; tests point
/// [`GitHub::at`] somewhere else.
const API: &str = "https://api.github.com";

/// The REST API version this module speaks, sent on every request so a
/// payload never changes shape under us without someone bumping this.
const API_VERSION: &str = "2022-11-28";

/// How long to wait for GitHub to answer. These are bounded request-response
/// exchanges, not streams, so the whole call gets a deadline.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A repository, named the way GitHub names one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    #[must_use]
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Open or closed. GitHub has no third state for issues or pull requests.
///
/// REST spells these `open`/`closed` and GraphQL shouts `OPEN`/`CLOSED`;
/// both spellings land here so the difference stays inside the module.
/// Serialization — a tool result on its way to a model — always spells them
/// the REST way.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum State {
    #[serde(rename = "open", alias = "OPEN")]
    Open,
    #[serde(rename = "closed", alias = "CLOSED")]
    Closed,
}

/// An issue, as Epik reads one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    /// GitHub sends `null` for an issue with no description; to Epik that is
    /// the same as an empty one.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub body: String,
    pub state: State,
}

/// One end of a pull request: a branch name and the commit it points at.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Branch {
    #[serde(rename = "ref")]
    pub name: String,
    pub sha: String,
}

/// A pull request, as Epik reads one. `head.sha` is what
/// [`check_conclusions`](GitHub::check_conclusions) wants.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub state: State,
    /// A merged pull request is `Closed` with this set; GitHub does not
    /// distinguish merged from closed in `state`.
    pub merged: bool,
    pub head: Branch,
    pub base: Branch,
}

/// How to merge a pull request. Deserializes from the same lowercase words
/// GitHub's own UI uses, which is how a tool argument names one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Merge {
    Commit,
    Squash,
    Rebase,
}

impl Merge {
    const fn wire(self) -> &'static str {
        match self {
            Self::Commit => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

/// One check run's verdict on a commit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Check {
    pub name: String,
    /// `None` while the check is still running.
    pub conclusion: Option<Conclusion>,
}

/// How a finished check run ended. `Other` absorbs conclusions GitHub
/// invents after this enum was written; an unmodelled verdict must not break
/// decoding, and it is nobody's green.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Conclusion {
    Success,
    Failure,
    Neutral,
    Cancelled,
    Skipped,
    Stale,
    TimedOut,
    ActionRequired,
    #[serde(other)]
    Other,
}

/// An issue as it appears at the far end of an edge: enough to schedule
/// against, not the whole issue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Edge {
    pub number: u64,
    pub title: String,
    pub state: State,
}

/// One issue and the edges around it: the children it decomposes into, and
/// the issues it waits on.
///
/// This is what the scheduler folds over — and, as a tool result, what a
/// model reads to answer for an issue's place in a plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IssueGraph {
    pub issue: Issue,
    pub sub_issues: Vec<Edge>,
    pub blocked_by: Vec<Edge>,
}

/// What went wrong, as a value. Doors render these; nothing here is meant to
/// be scraped out of a log.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// The verb needs a token — it writes, or it speaks GraphQL — and there
    /// is none. The ordinary state of a fresh install, and a card to render
    /// rather than a fault: paste a PAT, or set `EPIK_GITHUB_TOKEN`.
    TokenAbsent,
    /// GitHub said "come back later" rather than "no": a primary or
    /// secondary rate limit, in GitHub's own words. Retrying after a pause
    /// is legitimate; retrying [`Refused`](Self::Refused) is not.
    RateLimited(String),
    /// GitHub answered no, in its own words. A GraphQL refusal arrives under
    /// HTTP 200; `status` reports what the transport said either way.
    Refused { status: u16, message: String },
    /// The request never got an answer, or got one this client could not
    /// read. Names the endpoint it was reaching for.
    Wire(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TokenAbsent => write!(
                f,
                "no GitHub token: this needs one — paste a PAT into Epik, or set {}",
                crate::keystore::GITHUB_OVERRIDE_ENV
            ),
            Self::RateLimited(message) => write!(f, "GitHub rate limit: {message}"),
            Self::Refused { status, message } => {
                write!(f, "GitHub refused with {status}: {message}")
            }
            Self::Wire(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

/// GitHub, as Epik needs it: exactly the verbs the harness speaks, over one
/// authenticated agent.
#[derive(Debug)]
pub struct GitHub {
    api: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl GitHub {
    /// A client for GitHub itself, authenticating with `token` when there is
    /// one. A missing token is not an error here: public repositories read
    /// fine without one, and the verbs that do need one refuse individually.
    #[must_use]
    pub fn new(token: Option<String>) -> Self {
        Self::at(API, token)
    }

    /// [`new`](Self::new), aimed at a stated API base — which is how a test
    /// talks to a fake GitHub on a loopback port.
    #[must_use]
    pub fn at(api: impl Into<String>, token: Option<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            // Epik reports GitHub's own words, so it needs the body of a
            // refusal rather than just its status code.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            api: api.into().trim_end_matches('/').to_owned(),
            token,
            agent,
        }
    }

    // ----- the REST verbs -----

    /// The branch a pull request merges into by default.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when GitHub cannot be asked or answers no.
    pub fn default_branch(&self, repo: &Repo) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Wire {
            default_branch: String,
        }
        let wire: Wire = self.get(&format!("repos/{repo}"))?;
        Ok(wire.default_branch)
    }

    /// One issue, by number.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when GitHub cannot be asked or answers no.
    pub fn issue(&self, repo: &Repo, number: u64) -> Result<Issue, Error> {
        self.get(&format!("repos/{repo}/issues/{number}"))
    }

    /// Opens an issue.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn create_issue(&self, repo: &Repo, title: &str, body: &str) -> Result<Issue, Error> {
        self.needs_token()?;
        self.send(
            Method::Post,
            &format!("repos/{repo}/issues"),
            &json!({ "title": title, "body": body }),
        )
    }

    /// Rewrites an issue's title, body, or both. `None` leaves a field as it
    /// stands.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn edit_issue(
        &self,
        repo: &Repo,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<Issue, Error> {
        self.needs_token()?;
        let mut fields = serde_json::Map::new();
        if let Some(title) = title {
            fields.insert("title".to_owned(), json!(title));
        }
        if let Some(body) = body {
            fields.insert("body".to_owned(), json!(body));
        }
        self.send(
            Method::Patch,
            &format!("repos/{repo}/issues/{number}"),
            &Value::Object(fields),
        )
    }

    /// Adds a comment to an issue — or to a pull request, which is an issue
    /// to this endpoint.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn comment(&self, repo: &Repo, number: u64, body: &str) -> Result<(), Error> {
        self.needs_token()?;
        self.send::<Value>(
            Method::Post,
            &format!("repos/{repo}/issues/{number}/comments"),
            &json!({ "body": body }),
        )?;
        Ok(())
    }

    /// Closes an issue.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn close_issue(&self, repo: &Repo, number: u64) -> Result<Issue, Error> {
        self.needs_token()?;
        self.send(
            Method::Patch,
            &format!("repos/{repo}/issues/{number}"),
            &json!({ "state": "closed" }),
        )
    }

    /// Opens a pull request merging `head` into `base`.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn open_pull(
        &self,
        repo: &Repo,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
    ) -> Result<Pull, Error> {
        self.needs_token()?;
        self.send(
            Method::Post,
            &format!("repos/{repo}/pulls"),
            &json!({ "title": title, "head": head, "base": base, "body": body }),
        )
    }

    /// One pull request, by number.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when GitHub cannot be asked or answers no.
    pub fn pull(&self, repo: &Repo, number: u64) -> Result<Pull, Error> {
        self.get(&format!("repos/{repo}/pulls/{number}"))
    }

    /// Merges a pull request. A branch that is not mergeable — red checks, a
    /// ruleset, a conflict — comes back as [`Error::Refused`] in GitHub's
    /// own words.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn merge_pull(&self, repo: &Repo, number: u64, method: Merge) -> Result<(), Error> {
        self.needs_token()?;
        self.send::<Value>(
            Method::Put,
            &format!("repos/{repo}/pulls/{number}/merge"),
            &json!({ "merge_method": method.wire() }),
        )?;
        Ok(())
    }

    /// Every check run's verdict on `git_ref` — a branch name, a tag, or a
    /// commit SHA. "Is this green" is a fold over the result; a run still
    /// executing has no conclusion yet.
    ///
    /// One page of a hundred runs, unpaginated. Epik's own repositories run
    /// a handful of checks; a commit carrying more than a hundred would come
    /// back truncated, and a fold over a truncated list can call red green —
    /// so the cap is stated here, where a caller reads, until a repository
    /// exists that needs the pagination.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when GitHub cannot be asked or answers no.
    pub fn check_conclusions(&self, repo: &Repo, git_ref: &str) -> Result<Vec<Check>, Error> {
        #[derive(Deserialize)]
        struct Wire {
            check_runs: Vec<Check>,
        }
        let wire: Wire = self.get(&format!(
            "repos/{repo}/commits/{git_ref}/check-runs?per_page=100"
        ))?;
        Ok(wire.check_runs)
    }

    // ----- the GraphQL verbs -----

    /// One issue and its edges: sub-issues and blocked-by, which REST never
    /// learned to serve.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token — GraphQL
    /// answers nothing unauthenticated, even about a public repository.
    /// Otherwise an [`Error`] when GitHub cannot be asked or answers no.
    pub fn issue_graph(&self, repo: &Repo, number: u64) -> Result<IssueGraph, Error> {
        let data = self.graphql(GRAPH_QUERY, &variables(repo, number))?;
        graph(data)
    }

    /// Makes `child` a sub-issue of `parent`.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn add_sub_issue(&self, repo: &Repo, parent: u64, child: u64) -> Result<(), Error> {
        self.link(ADD_SUB_ISSUE, repo, parent, child)
    }

    /// Detaches `child` from `parent`. The issue survives; the edge does not.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn remove_sub_issue(&self, repo: &Repo, parent: u64, child: u64) -> Result<(), Error> {
        self.link(REMOVE_SUB_ISSUE, repo, parent, child)
    }

    /// Records that `issue` is blocked by `blocker`.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn add_blocked_by(&self, repo: &Repo, issue: u64, blocker: u64) -> Result<(), Error> {
        self.link(ADD_BLOCKED_BY, repo, issue, blocker)
    }

    /// Removes a blocked-by edge — what "unblocked" is.
    ///
    /// # Errors
    ///
    /// Refuses with [`Error::TokenAbsent`] when there is no token; otherwise
    /// an [`Error`] when GitHub cannot be asked or answers no.
    pub fn remove_blocked_by(&self, repo: &Repo, issue: u64, blocker: u64) -> Result<(), Error> {
        self.link(REMOVE_BLOCKED_BY, repo, issue, blocker)
    }

    // ----- plumbing -----

    /// The precondition of every verb that needs a token — the writes, and
    /// all of GraphQL — checked before any request goes out: a refusal for
    /// want of a token should cost nothing and change nothing.
    const fn needs_token(&self) -> Result<(), Error> {
        match self.token {
            Some(_) => Ok(()),
            None => Err(Error::TokenAbsent),
        }
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let url = format!("{}/{path}", self.api);
        let outcome = self
            .agent
            .get(&url)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", API_VERSION)
            .bearer(self.token.as_deref())
            .call();
        decode(&url, answer(&url, outcome)?)
    }

    fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &Value,
    ) -> Result<T, Error> {
        let url = format!("{}/{path}", self.api);
        let builder = match method {
            Method::Post => self.agent.post(&url),
            Method::Patch => self.agent.patch(&url),
            Method::Put => self.agent.put(&url),
        };
        let outcome = builder
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", API_VERSION)
            .bearer(self.token.as_deref())
            .send_json(body);
        decode(&url, answer(&url, outcome)?)
    }

    /// One GraphQL exchange: the query goes out with its variables, and what
    /// comes back is either `data` or a typed refusal. GraphQL reports
    /// failure in-band — HTTP 200 with an `errors` array — so this is where
    /// that shape is settled too.
    fn graphql(&self, query: &'static str, variables: &Value) -> Result<Value, Error> {
        self.needs_token()?;
        let url = format!("{}/graphql", self.api);
        let outcome = self
            .agent
            .post(&url)
            .bearer(self.token.as_deref())
            .send_json(json!({ "query": query, "variables": variables }));
        graphql_data(answer(&url, outcome)?)
    }

    /// The shared shape of every relationship mutation: resolve two issue
    /// numbers to node ids, then run `mutation` over them.
    fn link(&self, mutation: &'static str, repo: &Repo, from: u64, to: u64) -> Result<(), Error> {
        let from = self.issue_id(repo, from)?;
        let to = self.issue_id(repo, to)?;
        self.graphql(mutation, &json!({ "from": from, "to": to }))?;
        Ok(())
    }

    /// The GraphQL node id behind an issue number — what the relationship
    /// mutations take, and a name for an issue no caller ever sees.
    fn issue_id(&self, repo: &Repo, number: u64) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Data {
            repository: Repository,
        }
        #[derive(Deserialize)]
        struct Repository {
            issue: Node,
        }
        #[derive(Deserialize)]
        struct Node {
            id: String,
        }
        let data = self.graphql(ID_QUERY, &variables(repo, number))?;
        let data: Data = decode("the issue id", data)?;
        Ok(data.repository.issue.id)
    }
}

/// The methods this module writes with. A private vocabulary, so the verb
/// implementations stay one-liners over [`GitHub::send`].
#[derive(Clone, Copy, Debug)]
enum Method {
    Post,
    Patch,
    Put,
}

/// The one extension trait this module allows itself: attaching the
/// authorization header to either kind of request builder.
trait Bearer {
    #[must_use]
    fn bearer(self, token: Option<&str>) -> Self;
}

impl<B> Bearer for ureq::RequestBuilder<B> {
    fn bearer(self, token: Option<&str>) -> Self {
        match token {
            Some(token) => self.header("authorization", format!("Bearer {token}")),
            None => self,
        }
    }
}

/// One response, settled: a success's decoded body, or the typed refusal
/// the status and rate-limit headers add up to.
fn answer(
    url: &str,
    outcome: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<Value, Error> {
    let mut response = outcome.map_err(|error| Error::Wire(format!("asking {url}: {error}")))?;
    let status = response.status().as_u16();
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| Error::Wire(format!("reading {url}'s answer: {error}")))?;
    if (200..300).contains(&status) {
        if body.is_empty() {
            return Ok(Value::Null);
        }
        return serde_json::from_str(&body)
            .map_err(|error| Error::Wire(format!("decoding {url}'s answer: {error}")));
    }
    Err(refusal(status, remaining.as_deref(), &body))
}

/// Classifies a non-success REST answer. Rate limiting is recognized two
/// ways, because GitHub reports it two ways: a primary limit is a 403 or 429
/// with `x-ratelimit-remaining: 0`, and a secondary limit is a 403 whose
/// message says so with a nonzero remaining count.
fn refusal(status: u16, remaining: Option<&str>, body: &str) -> Error {
    let message = message(body);
    let exhausted = remaining == Some("0");
    let named = message.to_lowercase().contains("rate limit");
    if status == 429 || (status == 403 && (exhausted || named)) {
        return Error::RateLimited(message);
    }
    Error::Refused { status, message }
}

/// GitHub's own words for a refusal: the `message` field when the body is
/// the usual error shape, the raw body when it is not.
fn message(body: &str) -> String {
    #[derive(Deserialize)]
    struct Wire {
        message: String,
    }
    serde_json::from_str::<Wire>(body).map_or_else(|_| body.trim().to_owned(), |wire| wire.message)
}

/// Settles a GraphQL reply into its `data`, or into the typed refusal its
/// `errors` array amounts to. `RATE_LIMITED` is in-band here, not a status
/// code, which is why the classification cannot live with the REST one.
fn graphql_data(reply: Value) -> Result<Value, Error> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default)]
        data: Value,
        #[serde(default)]
        errors: Vec<GraphqlError>,
    }
    #[derive(Deserialize)]
    struct GraphqlError {
        #[serde(default, rename = "type")]
        kind: String,
        message: String,
    }
    let wire: Wire = decode("the GraphQL reply", reply)?;
    if wire.errors.is_empty() {
        return Ok(wire.data);
    }
    let messages = wire
        .errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    if wire.errors.iter().any(|error| error.kind == "RATE_LIMITED") {
        return Err(Error::RateLimited(messages));
    }
    Err(Error::Refused {
        status: 200,
        message: messages,
    })
}

/// Decodes the graph query's `data` into the type the scheduler folds over.
fn graph(data: Value) -> Result<IssueGraph, Error> {
    #[derive(Deserialize)]
    struct Data {
        repository: Repository,
    }
    #[derive(Deserialize)]
    struct Repository {
        issue: Wire,
    }
    #[derive(Deserialize)]
    struct Wire {
        number: u64,
        title: String,
        state: State,
        #[serde(default, deserialize_with = "null_is_empty")]
        body: String,
        #[serde(rename = "subIssues")]
        sub_issues: Connection,
        #[serde(rename = "blockedBy")]
        blocked_by: Connection,
    }
    #[derive(Deserialize)]
    struct Connection {
        nodes: Vec<Edge>,
    }
    let data: Data = decode("the issue graph", data)?;
    let wire = data.repository.issue;
    Ok(IssueGraph {
        issue: Issue {
            number: wire.number,
            title: wire.title,
            body: wire.body,
            state: wire.state,
        },
        sub_issues: wire.sub_issues.nodes,
        blocked_by: wire.blocked_by.nodes,
    })
}

fn decode<T: DeserializeOwned>(what: &str, value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|error| Error::Wire(format!("decoding {what}: {error}")))
}

fn null_is_empty<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// The variables every per-issue query takes.
fn variables(repo: &Repo, number: u64) -> Value {
    json!({ "owner": repo.owner, "name": repo.name, "number": number })
}

// The GraphQL strings. Sub-issues and blocked-by have no REST endpoints, and
// each fetch asks for the first hundred edges without paginating: GitHub
// itself caps sub-issues at one hundred per parent, and an issue blocked by
// more than a hundred others has problems no client can fix.

const ID_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { \
     repository(owner: $owner, name: $name) { issue(number: $number) { id } } }";

const GRAPH_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { \
     repository(owner: $owner, name: $name) { issue(number: $number) { \
     number title state body \
     subIssues(first: 100) { nodes { number title state } } \
     blockedBy(first: 100) { nodes { number title state } } } } }";

const ADD_SUB_ISSUE: &str = "mutation($from: ID!, $to: ID!) { \
     addSubIssue(input: {issueId: $from, subIssueId: $to}) { clientMutationId } }";

const REMOVE_SUB_ISSUE: &str = "mutation($from: ID!, $to: ID!) { \
     removeSubIssue(input: {issueId: $from, subIssueId: $to}) { clientMutationId } }";

const ADD_BLOCKED_BY: &str = "mutation($from: ID!, $to: ID!) { \
     addBlockedBy(input: {issueId: $from, blockingIssueId: $to}) { clientMutationId } }";

const REMOVE_BLOCKED_BY: &str = "mutation($from: ID!, $to: ID!) { \
     removeBlockedBy(input: {issueId: $from, blockingIssueId: $to}) { clientMutationId } }";

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured payloads from the real API — this repository, read while it
    /// stood at the commits the fixtures record. See `fixtures/README.md`
    /// for provenance and recapture commands.
    const ISSUE: &str = include_str!("github/fixtures/issue.json");
    const PULL: &str = include_str!("github/fixtures/pull.json");
    const REPO: &str = include_str!("github/fixtures/repo.json");
    const CHECK_RUNS: &str = include_str!("github/fixtures/check_runs.json");
    const GRAPH_PARENT: &str = include_str!("github/fixtures/graph_parent.json");
    const GRAPH_BLOCKED: &str = include_str!("github/fixtures/graph_blocked.json");

    fn value(payload: &str) -> Value {
        serde_json::from_str(payload).unwrap()
    }

    fn epik() -> Repo {
        Repo::new("epik-agent", "Epik")
    }

    #[test]
    fn a_captured_issue_decodes() {
        let issue: Issue = decode("an issue", value(ISSUE)).unwrap();
        assert_eq!(issue.number, 106);
        assert!(issue.title.contains("typed REST + GraphQL client"));
        assert!(issue.body.contains("hand-rolled client"));
        assert_eq!(issue.state, State::Open);
    }

    #[test]
    fn an_issue_with_no_body_decodes_to_an_empty_one() {
        let payload = r#"{"number": 7, "title": "t", "body": null, "state": "open"}"#;
        let issue: Issue = decode("an issue", value(payload)).unwrap();
        assert_eq!(issue.body, "");
    }

    #[test]
    fn a_captured_pull_decodes_with_its_branches() {
        let pull: Pull = decode("a pull", value(PULL)).unwrap();
        assert_eq!(pull.number, 91);
        assert_eq!(pull.state, State::Closed);
        assert!(pull.merged, "PR #91 merged, and the fixture says so");
        assert_eq!(pull.head.name, "local-client-greenfield");
        assert_eq!(pull.base.name, "main");
        assert_eq!(pull.head.sha.len(), 40, "{}", pull.head.sha);
    }

    #[test]
    fn a_captured_repo_yields_its_default_branch() {
        #[derive(Deserialize)]
        struct Wire {
            default_branch: String,
        }
        let wire: Wire = decode("a repo", value(REPO)).unwrap();
        assert_eq!(wire.default_branch, "main");
    }

    #[test]
    fn captured_check_runs_decode_with_their_conclusions() {
        #[derive(Deserialize)]
        struct Wire {
            check_runs: Vec<Check>,
        }
        let wire: Wire = decode("check runs", value(CHECK_RUNS)).unwrap();
        assert_eq!(wire.check_runs.len(), 5);
        assert!(
            wire.check_runs
                .iter()
                .all(|check| check.conclusion == Some(Conclusion::Success)),
            "the captured commit was green: {:?}",
            wire.check_runs
        );
        assert!(
            wire.check_runs.iter().any(|check| check.name == "CI"),
            "the aggregate check is among them"
        );
    }

    #[test]
    fn a_check_still_running_has_no_conclusion() {
        let payload = r#"{"name": "CI", "conclusion": null}"#;
        let check: Check = decode("a check", value(payload)).unwrap();
        assert_eq!(check.conclusion, None);
    }

    #[test]
    fn a_conclusion_from_next_year_still_decodes() {
        let payload = r#"{"name": "CI", "conclusion": "quantum_flaked"}"#;
        let check: Check = decode("a check", value(payload)).unwrap();
        assert_eq!(check.conclusion, Some(Conclusion::Other));
    }

    #[test]
    fn a_captured_graph_decodes_its_sub_issues() {
        let graph = graph(graphql_data(value(GRAPH_PARENT)).unwrap()).unwrap();
        assert_eq!(graph.issue.number, 102);
        assert_eq!(graph.issue.state, State::Open);
        let numbers: Vec<u64> = graph.sub_issues.iter().map(|edge| edge.number).collect();
        assert_eq!(numbers, [105, 106]);
        assert!(graph.blocked_by.is_empty());
    }

    #[test]
    fn a_captured_graph_decodes_its_blocked_by_edges() {
        let graph = graph(graphql_data(value(GRAPH_BLOCKED)).unwrap()).unwrap();
        assert_eq!(graph.issue.number, 106);
        assert_eq!(graph.blocked_by.len(), 1);
        assert_eq!(graph.blocked_by[0].number, 105);
        assert_eq!(
            graph.blocked_by[0].state,
            State::Open,
            "GraphQL's OPEN lands on the same State as REST's open"
        );
        assert!(graph.sub_issues.is_empty());
    }

    #[test]
    fn a_primary_rate_limit_is_told_apart_from_a_refusal() {
        let body = r#"{"message": "API rate limit exceeded for 1.2.3.4."}"#;
        assert_eq!(
            refusal(403, Some("0"), body),
            Error::RateLimited("API rate limit exceeded for 1.2.3.4.".to_owned())
        );
        assert_eq!(
            refusal(429, Some("42"), body),
            Error::RateLimited("API rate limit exceeded for 1.2.3.4.".to_owned()),
            "a 429 is a rate limit whatever the headers say"
        );
    }

    #[test]
    fn a_secondary_rate_limit_names_itself_rather_than_exhausting_the_count() {
        let body = r#"{"message": "You have exceeded a secondary rate limit."}"#;
        assert!(matches!(
            refusal(403, Some("55"), body),
            Error::RateLimited(_)
        ));
    }

    #[test]
    fn a_plain_403_is_a_refusal_in_githubs_own_words() {
        let body = r#"{"message": "Resource not accessible by personal access token"}"#;
        assert_eq!(
            refusal(403, Some("55"), body),
            Error::Refused {
                status: 403,
                message: "Resource not accessible by personal access token".to_owned()
            }
        );
    }

    #[test]
    fn a_refusal_with_an_unusual_body_reports_the_body_itself() {
        assert_eq!(
            refusal(502, None, "Bad Gateway\n"),
            Error::Refused {
                status: 502,
                message: "Bad Gateway".to_owned()
            }
        );
    }

    #[test]
    fn a_graphql_rate_limit_is_in_band_and_still_told_apart() {
        let reply = value(
            r#"{"data": null, "errors": [
                {"type": "RATE_LIMITED", "message": "API rate limit exceeded"}]}"#,
        );
        assert_eq!(
            graphql_data(reply),
            Err(Error::RateLimited("API rate limit exceeded".to_owned()))
        );
    }

    #[test]
    fn a_graphql_error_is_a_refusal_in_githubs_own_words() {
        let reply = value(
            r#"{"data": {"repository": null}, "errors": [
                {"type": "NOT_FOUND", "message": "Could not resolve to an Issue"}]}"#,
        );
        assert_eq!(
            graphql_data(reply),
            Err(Error::Refused {
                status: 200,
                message: "Could not resolve to an Issue".to_owned()
            })
        );
    }

    #[test]
    fn a_clean_graphql_reply_yields_its_data() {
        let reply = value(r#"{"data": {"repository": {"issue": {"id": "I_abc"}}}}"#);
        let data = graphql_data(reply).unwrap();
        assert_eq!(data["repository"]["issue"]["id"], "I_abc");
    }

    #[test]
    fn every_writing_verb_refuses_without_a_token_before_any_wire_is_touched() {
        // Port 1 is reserved and nothing may listen there, so a verb that
        // reached for the network would fail with Wire, not TokenAbsent.
        let github = GitHub::at("http://127.0.0.1:1", None);
        let repo = epik();
        let refusals = [
            github.create_issue(&repo, "t", "b").map(drop),
            github.edit_issue(&repo, 1, Some("t"), None).map(drop),
            github.comment(&repo, 1, "b"),
            github.close_issue(&repo, 1).map(drop),
            github.open_pull(&repo, "t", "h", "b", "").map(drop),
            github.merge_pull(&repo, 1, Merge::Squash),
            github.issue_graph(&repo, 1).map(drop),
            github.add_sub_issue(&repo, 1, 2),
            github.remove_sub_issue(&repo, 1, 2),
            github.add_blocked_by(&repo, 1, 2),
            github.remove_blocked_by(&repo, 1, 2),
        ];
        for refusal in refusals {
            assert_eq!(refusal, Err(Error::TokenAbsent));
        }
    }

    #[test]
    fn the_refusal_for_a_missing_token_says_how_to_supply_one() {
        let rendered = Error::TokenAbsent.to_string();
        assert!(
            rendered.contains(crate::keystore::GITHUB_OVERRIDE_ENV),
            "{rendered}"
        );
    }

    #[test]
    fn an_unreachable_endpoint_says_where_it_tried() {
        let github = GitHub::at("http://127.0.0.1:1", None);
        let error = github.issue(&epik(), 106).unwrap_err();
        let Error::Wire(message) = error else {
            panic!("nothing listens on port 1: {error:?}");
        };
        assert!(
            message.contains("http://127.0.0.1:1/repos/epik-agent/Epik/issues/106"),
            "a failure should name the endpoint it was reaching for: {message}"
        );
    }

    #[test]
    fn the_api_base_keeps_exactly_one_slash_before_the_path() {
        for base in ["http://localhost:9", "http://localhost:9/"] {
            assert_eq!(GitHub::at(base, None).api, "http://localhost:9");
        }
    }

    #[test]
    fn a_repo_prints_the_way_github_spells_one() {
        assert_eq!(epik().to_string(), "epik-agent/Epik");
    }
}
