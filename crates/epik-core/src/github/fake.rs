//! A fake GitHub on a loopback port: one repository's branches, issues,
//! pull requests, check runs, and commit statuses, held in memory and
//! served over just enough of the REST API for a feature build.
//!
//! A test arranges the [`World`], points a real [`GitHub`] at the fake,
//! and afterwards reads back the world as the calls left it and every
//! [`Request`] received, in order. A request the fake has no route for
//! answers 404 in GitHub's shape.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};

use super::{Branch, Check, Conclusion, GitHub, Issue, Pull, Repo, State};

/// The default branch.
pub const DEFAULT_BRANCH: &str = "main";
/// The commit the default branch stands on when the fake starts.
pub const TIP: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// One request as the fake received it. The path carries its query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Value,
}

impl Request {
    pub fn new(method: &str, path: &str, body: Value) -> Self {
        Self {
            method: method.to_owned(),
            path: path.to_owned(),
            body,
        }
    }
}

/// The commit a push left a branch on, read from wherever the test's
/// pushes land.
pub type Pushed = Box<dyn Fn(&str) -> Option<String> + Send>;

/// What the fake repository holds.
#[derive(Default)]
pub struct World {
    /// Each branch and the commit it stands on.
    pub branches: BTreeMap<String, String>,
    /// Where a branch [`branches`](Self::branches) does not name stands,
    /// for a test whose pushes land somewhere the fake can read: what a
    /// push reaching GitHub leaves behind.
    pub pushes_to: Option<Pushed>,
    pub issues: BTreeMap<u64, Issue>,
    /// Pull request `n` is `pulls[n - 1]`.
    pub pulls: Vec<Pull>,
    /// Each commit's check runs, as the next read finds them.
    pub checks: BTreeMap<String, Vec<Check>>,
    /// Check runs a commit gains once its runs have been read that many
    /// times: GitHub creates a job's run when the job is queued, so a
    /// run whose job waits on another appears mid-wait.
    pub appearing: Vec<(String, usize, Check)>,
    /// How many times each commit's check runs have been read.
    pub reads: BTreeMap<String, usize>,
    /// How many commit statuses each commit carries.
    pub statuses: BTreeMap<String, usize>,
    /// GitHub's words for refusing every merge, when it refuses.
    pub merge_refusal: Option<String>,
    /// The rulesets the repository holds, each over the branches its
    /// pattern names.
    pub rulesets: Vec<Ruleset>,
    pub requests: Vec<Request>,
}

/// One ruleset the fake serves: an id, the account it belongs to, the
/// branch names it covers, and its rules as GitHub sends them.
pub struct Ruleset {
    pub id: u64,
    pub source: String,
    /// The branch names the ruleset covers. A trailing `*` stands for any
    /// ending, which is the whole of GitHub's `fnmatch` a build's branches
    /// are ever matched by.
    pub pattern: String,
    pub rules: Vec<Value>,
}

impl Ruleset {
    /// A ruleset numbered `id` over the branches `pattern` names, holding
    /// `rules`, belonging to the repository the fake serves.
    pub fn new(id: u64, pattern: &str, rules: Vec<Value>) -> Self {
        Self {
            id,
            source: "fake/repo".to_owned(),
            pattern: pattern.to_owned(),
            rules,
        }
    }

    /// Whether the ruleset covers `branch`.
    fn covers(&self, branch: &str) -> bool {
        match self.pattern.strip_suffix('*') {
            Some(stem) => branch.starts_with(stem),
            None => branch == self.pattern,
        }
    }
}

/// A check run named `name`, still running when `conclusion` is `None`,
/// reported by no app.
pub fn check(name: &str, conclusion: Option<Conclusion>) -> Check {
    Check {
        name: name.to_owned(),
        conclusion,
        app: None,
    }
}

/// A [`check`] the app numbered `app` reported.
pub fn check_by(name: &str, conclusion: Option<Conclusion>, app: u64) -> Check {
    Check {
        app: Some(app),
        ..check(name, conclusion)
    }
}

/// The read of commit `sha`'s check runs, as the fake receives it.
pub fn check_runs(sha: &str) -> Request {
    let path = format!("/repos/fake/repo/commits/{sha}/check-runs?per_page=100");
    Request::new("GET", &path, Value::Null)
}

/// The read of `branch`'s rules, as the fake receives it.
pub fn branch_rules(branch: &str) -> Request {
    let path = format!("/repos/fake/repo/rules/branches/{branch}");
    Request::new("GET", &path, Value::Null)
}

/// The fake, listening on loopback until the process ends.
pub struct FakeGitHub {
    url: String,
    world: Arc<Mutex<World>>,
}

impl FakeGitHub {
    /// A repository whose default branch stands on [`TIP`], served on a
    /// thread of its own.
    pub fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let world = Arc::new(Mutex::new(World::default()));
        let fake = Self { url, world };
        fake.world()
            .branches
            .insert(DEFAULT_BRANCH.to_owned(), TIP.to_owned());
        let world = Arc::clone(&fake.world);
        std::thread::spawn(move || {
            while let Ok((mut socket, _)) = listener.accept() {
                let Some(request) = read_request(&mut socket) else {
                    continue;
                };
                let (status, body) = {
                    let mut world = world.lock().unwrap_or_else(PoisonError::into_inner);
                    world.requests.push(request.clone());
                    world.answer(&request)
                };
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status} Fake\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        fake
    }

    /// A client aimed at the fake, carrying a token so the writing verbs
    /// go out.
    pub fn github(&self) -> GitHub {
        GitHub::at(&self.url, Some("ghp_fake".into()))
    }

    /// The repository the fake serves.
    pub fn repo() -> Repo {
        Repo::new("fake", "repo")
    }

    /// The world, to arrange or to read back.
    pub fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.world().requests.clone()
    }
}

/// GitHub's shape for a refusal.
fn refused(status: u16, message: &str) -> (u16, Value) {
    (status, json!({ "message": message }))
}

impl World {
    /// An open issue titled `title`.
    pub fn open_issue(&mut self, number: u64, title: &str) {
        let issue = Issue {
            number,
            title: title.to_owned(),
            body: String::new(),
            state: State::Open,
        };
        self.issues.insert(number, issue);
    }

    /// The status and body GitHub would answer `request` with, and the
    /// world changed as the request asks.
    fn answer(&mut self, request: &Request) -> (u16, Value) {
        let path = request.path.split('?').next().unwrap_or_default();
        let Some(path) = path.strip_prefix("/repos/fake/repo") else {
            return refused(404, "Not Found");
        };
        let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let answer = match (request.method.as_str(), segments.as_slice()) {
            ("GET", []) => Some((200, json!({ "default_branch": DEFAULT_BRANCH }))),
            ("GET", ["git", "ref", "heads", branch]) => self
                .sha_of(branch)
                .map(|sha| (200, json!({ "object": { "sha": sha } }))),
            ("POST", ["git", "refs"]) => Some(self.create_branch(&request.body)),
            ("GET", ["issues", number]) => self.issue(number).map(|issue| (200, json!(issue))),
            ("PATCH", ["issues", number]) => self.issue(number).map(|issue| {
                if let Ok(state) = serde_json::from_value(request.body["state"].clone()) {
                    issue.state = state;
                }
                (200, json!(issue))
            }),
            ("POST", ["pulls"]) => Some(self.open_pull(&request.body)),
            ("GET", ["pulls"]) => Some(self.pulls_for(&request.path)),
            ("GET", ["pulls", number]) => self.pull(number).map(|pull| (200, json!(pull))),
            ("PUT", ["pulls", number, "merge"]) => {
                let refusal = self.merge_refusal.clone();
                let asked = request.body["sha"].as_str().map(str::to_owned);
                // The head is where the branch now stands, as GitHub's
                // own is: a push onto an open pull request moves it.
                let opened = self
                    .pull(number)
                    .map(|pull| (pull.head.name.clone(), pull.head.sha.clone()));
                let head = opened.map(|(branch, at_open)| self.sha_of(&branch).unwrap_or(at_open));
                self.pull(number).map(|pull| match (refusal, asked) {
                    (Some(words), _) => refused(405, &words),
                    (None, Some(sha)) if Some(&sha) != head.as_ref() => refused(
                        409,
                        "Head branch was modified. Review and try the merge again.",
                    ),
                    (None, _) => {
                        if let Some(head) = head {
                            pull.head.sha = head;
                        }
                        pull.merged = true;
                        pull.state = State::Closed;
                        (200, json!({ "merged": true }))
                    }
                })
            }
            ("GET", ["commits", sha, "check-runs"]) => {
                let read = self.reads.entry((*sha).to_owned()).or_default();
                *read += 1;
                let read = *read;
                let appearing = self.appearing.iter();
                let due: Vec<Check> = appearing
                    .filter(|(at, after, _)| at == sha && read > *after)
                    .map(|(_, _, run)| run.clone())
                    .collect();
                let mut runs = self.checks.get(*sha).cloned().unwrap_or_default();
                runs.extend(due);
                Some((
                    200,
                    json!({ "total_count": runs.len(), "check_runs": runs }),
                ))
            }
            ("GET", ["rules", "branches", branch]) => Some((200, self.rules_for(branch))),
            ("GET", ["commits", sha, "status"]) => {
                let count = self.statuses.get(*sha).copied().unwrap_or(0);
                Some((200, json!({ "state": "pending", "total_count": count })))
            }
            _ => None,
        };
        answer.unwrap_or_else(|| refused(404, "Not Found"))
    }

    /// The rules in force on `branch`: every covering ruleset's rules,
    /// each stamped with the ruleset it came from, as GitHub flattens
    /// them. A branch no ruleset covers has an empty list, whether or not
    /// the branch exists.
    fn rules_for(&self, branch: &str) -> Value {
        let flattened: Vec<Value> = self
            .rulesets
            .iter()
            .filter(|ruleset| ruleset.covers(branch))
            .flat_map(|ruleset| {
                ruleset.rules.iter().map(|rule| {
                    let mut rule = rule.clone();
                    rule["ruleset_source_type"] = json!("Repository");
                    rule["ruleset_source"] = json!(ruleset.source);
                    rule["ruleset_id"] = json!(ruleset.id);
                    rule
                })
            })
            .collect();
        json!(flattened)
    }

    /// The commit `branch` stands on: what the world holds, or, for a
    /// branch it does not name, where a push left it.
    fn sha_of(&self, branch: &str) -> Option<String> {
        let held = self.branches.get(branch).cloned();
        held.or_else(|| self.pushes_to.as_ref().and_then(|pushed| pushed(branch)))
    }

    fn issue(&mut self, number: &str) -> Option<&mut Issue> {
        self.issues.get_mut(&number.parse().ok()?)
    }

    fn pull(&mut self, number: &str) -> Option<&mut Pull> {
        let number: usize = number.parse().ok()?;
        self.pulls.get_mut(number.checked_sub(1)?)
    }

    fn create_branch(&mut self, body: &Value) -> (u16, Value) {
        let (Some(name), Some(sha)) = (
            body["ref"]
                .as_str()
                .and_then(|name| name.strip_prefix("refs/heads/")),
            body["sha"].as_str(),
        ) else {
            return refused(422, "Validation Failed");
        };
        if self.branches.contains_key(name) {
            return refused(422, "Reference already exists");
        }
        self.branches.insert(name.to_owned(), sha.to_owned());
        (201, json!({ "ref": body["ref"], "object": { "sha": sha } }))
    }

    /// The pull requests `path`'s `head=<owner>:<branch>` produced,
    /// newest first, as GitHub's list endpoint answers.
    fn pulls_for(&self, path: &str) -> (u16, Value) {
        let query = path.split('?').nth(1).unwrap_or_default();
        let head = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == "head")
            .and_then(|(_, value)| value.split_once("%3A").or_else(|| value.split_once(':')));
        let Some((_, branch)) = head else {
            return refused(422, "Validation Failed");
        };
        let found: Vec<&Pull> = self
            .pulls
            .iter()
            .rev()
            .filter(|pull| pull.head.name == branch)
            .collect();
        (200, json!(found))
    }

    fn open_pull(&mut self, body: &Value) -> (u16, Value) {
        let end = |key: &str| {
            let name = body[key].as_str()?;
            Some(Branch {
                name: name.to_owned(),
                sha: self.sha_of(name)?,
            })
        };
        let (Some(head), Some(base)) = (end("head"), end("base")) else {
            return refused(422, "Validation Failed");
        };
        let standing = self
            .pulls
            .iter()
            .find(|pull| pull.head.name == head.name && pull.state == State::Open);
        if let Some(pull) = standing {
            let words = format!("A pull request already exists for fake:{}.", pull.head.name);
            return refused(422, &words);
        }
        let pull = Pull {
            number: self.pulls.len() as u64 + 1,
            title: body["title"].as_str().unwrap_or_default().to_owned(),
            state: State::Open,
            merged: false,
            head,
            base,
        };
        self.pulls.push(pull.clone());
        (201, json!(pull))
    }
}

/// One HTTP request off `socket`: the request line, then
/// content-length's worth of JSON body, `Null` when there is none.
fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).ok()?;
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    let mut line = head.lines().next()?.split(' ');
    let (method, path) = (line.next()?, line.next()?);
    let length: usize = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, length)| length.trim().parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    socket.read_exact(&mut body).ok()?;
    Some(Request::new(
        method,
        path,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    ))
}
