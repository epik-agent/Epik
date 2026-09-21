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

/// What the fake repository holds.
#[derive(Debug, Default)]
pub struct World {
    /// Each branch and the commit it stands on.
    pub branches: BTreeMap<String, String>,
    pub issues: BTreeMap<u64, Issue>,
    /// Pull request `n` is `pulls[n - 1]`.
    pub pulls: Vec<Pull>,
    /// Each commit's check runs, as the next read finds them.
    pub checks: BTreeMap<String, Vec<Check>>,
    /// How many commit statuses each commit carries.
    pub statuses: BTreeMap<String, usize>,
    /// GitHub's words for refusing every merge, when it refuses.
    pub merge_refusal: Option<String>,
    pub requests: Vec<Request>,
}

/// A check run named `name`, still running when `conclusion` is `None`.
pub fn check(name: &str, conclusion: Option<Conclusion>) -> Check {
    Check {
        name: name.to_owned(),
        conclusion,
    }
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
                .branches
                .get(*branch)
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
            ("GET", ["pulls", number]) => self.pull(number).map(|pull| (200, json!(pull))),
            ("PUT", ["pulls", number, "merge"]) => {
                let refusal = self.merge_refusal.clone();
                self.pull(number).map(|pull| match refusal {
                    Some(words) => refused(405, &words),
                    None => {
                        pull.merged = true;
                        pull.state = State::Closed;
                        (200, json!({ "merged": true }))
                    }
                })
            }
            ("GET", ["commits", sha, "check-runs"]) => {
                let runs = self.checks.get(*sha).cloned().unwrap_or_default();
                Some((
                    200,
                    json!({ "total_count": runs.len(), "check_runs": runs }),
                ))
            }
            ("GET", ["commits", sha, "status"]) => {
                let count = self.statuses.get(*sha).copied().unwrap_or(0);
                Some((200, json!({ "state": "pending", "total_count": count })))
            }
            _ => None,
        };
        answer.unwrap_or_else(|| refused(404, "Not Found"))
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

    fn open_pull(&mut self, body: &Value) -> (u16, Value) {
        let end = |key: &str| {
            let name = body[key].as_str()?;
            Some(Branch {
                name: name.to_owned(),
                sha: self.branches.get(name)?.clone(),
            })
        };
        let (Some(head), Some(base)) = (end("head"), end("base")) else {
            return refused(422, "Validation Failed");
        };
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
