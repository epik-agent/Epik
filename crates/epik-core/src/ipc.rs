//! The app's local socket, through which other programs, chiefly Epik's MCP
//! server, start builds and read their state.
//!
//! The socket is `~/.epik/epik.sock` on macOS and Linux, and the named pipe
//! `epik` on Windows. Each connection carries one request, a line of JSON,
//! and gets one line of JSON in reply. `docs/app-protocol.md` describes the
//! protocol for clients.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, Listener, ListenerNonblockingMode, ListenerOptions, Name,
    Stream,
};
use serde::{Deserialize, Serialize};

use crate::builds::{Builds, Started};
use crate::record::{self, RunState};

/// How many runs `list` returns when it isn't given a limit.
pub const DEFAULT_LIMIT: usize = 20;

/// The longest request read. A build request is a few hundred bytes.
const MAX_REQUEST: u64 = 64 * 1024;

/// How long a client has to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a waiting server looks at its stop signal.
const STOP_POLL: Duration = Duration::from_millis(50);

/// What a client asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Start building an epic or an issue.
    Build {
        repo: String,
        number: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        plugin_dir: PathBuf,
    },
    /// A run's state.
    Status { run_id: String },
    /// The latest runs, newest first.
    List {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Whether the app is there, and its version.
    Ping,
}

/// What the app replies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The build has started: its run has written `run_started`.
    Started(Started),
    /// A run's state, folded from its events.
    Status(RunState),
    Runs {
        runs: Vec<RunState>,
    },
    Pong {
        version: String,
    },
    Error {
        message: String,
    },
}

impl Response {
    fn error(message: impl Into<String>) -> Self {
        Response::Error {
            message: message.into(),
        }
    }
}

/// Answers `request`.
fn handle(builds: &Builds, request: Request) -> Response {
    match request {
        Request::Build {
            repo,
            number,
            branch,
            plugin_dir,
        } => match builds.start(&repo, number, branch, plugin_dir) {
            Ok(started) => Response::Started(started),
            Err(why) => Response::error(format!("couldn't build {repo}#{number}: {why}")),
        },
        Request::Status { run_id } => status(builds.root(), &run_id),
        Request::List { limit } => match record::list_runs(builds.root()) {
            Ok(mut runs) => {
                runs.truncate(limit.unwrap_or(DEFAULT_LIMIT));
                Response::Runs { runs }
            }
            Err(error) => Response::error(format!("couldn't list the runs: {error}")),
        },
        Request::Ping => Response::Pong {
            version: crate::version().to_owned(),
        },
    }
}

/// Answers `line`, a request as JSON.
fn respond(builds: &Builds, line: &str) -> Response {
    match serde_json::from_str(line) {
        Ok(request) => handle(builds, request),
        Err(error) => Response::error(format!("bad request: {error}")),
    }
}

fn status(root: &Path, run_id: &str) -> Response {
    match record::read_events(root, run_id) {
        Ok(events) => match record::fold(&events) {
            Some(state) => Response::Status(state),
            None => Response::error(format!("run {run_id} hasn't started")),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Response::error(format!("no run {run_id:?}"))
        }
        Err(error) => Response::error(format!("couldn't read run {run_id}: {error}")),
    }
}

/// Where the socket is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// A socket file, on macOS and Linux.
    Path(PathBuf),
    /// A named pipe, on Windows.
    Pipe(String),
}

impl Address {
    /// The app's socket for Epik's root `root`: `<root>/epik.sock` on macOS
    /// and Linux, the named pipe `epik` on Windows.
    pub fn in_root(root: &Path) -> Self {
        if cfg!(windows) {
            Address::Pipe("epik".to_owned())
        } else {
            Address::Path(root.join("epik.sock"))
        }
    }

    fn name(&self) -> io::Result<Name<'_>> {
        match self {
            Address::Path(path) => path.as_path().to_fs_name::<GenericFilePath>(),
            Address::Pipe(pipe) => pipe.as_str().to_ns_name::<GenericNamespaced>(),
        }
    }
}

/// The app's end of the socket.
pub struct Server {
    listener: Listener,
}

impl Server {
    /// Listens at `address`, which only this user can open. A socket file
    /// left there by an app that is no longer running is removed; one an
    /// app is still serving is an [`io::ErrorKind::AddrInUse`] error.
    pub fn bind(address: &Address) -> io::Result<Self> {
        let Address::Path(path) = address else {
            return Ok(Self {
                listener: listen(address)?,
            });
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let listener = match listen(address) {
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                if Stream::connect(address.name()?).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!("an Epik app is already serving {}", path.display()),
                    ));
                }
                fs::remove_file(path)?;
                listen(address)?
            }
            listening => listening?,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self { listener })
    }

    /// Answers every connection, each on its own thread, until `stop` is
    /// set.
    pub fn serve(&self, builds: Arc<Builds>, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            match self.listener.accept() {
                Ok(stream) => {
                    let builds = builds.clone();
                    thread::spawn(move || {
                        // A client that goes away gets no answer.
                        let _ = converse(&builds, stream);
                    });
                }
                // Nothing is waiting, or a client gave up before it was
                // accepted.
                Err(_) => thread::sleep(STOP_POLL),
            }
        }
    }
}

/// Listens at `address`, with an accept that doesn't block so the server
/// can see its stop signal.
fn listen(address: &Address) -> io::Result<Listener> {
    ListenerOptions::new()
        .name(address.name()?)
        .nonblocking(ListenerNonblockingMode::Accept)
        .create_sync()
}

/// Reads one request from `stream` and writes its answer.
fn converse(builds: &Builds, mut stream: Stream) -> io::Result<()> {
    // Not every platform supports a timeout; without one, a silent client
    // only holds its own thread.
    let _ = stream.set_recv_timeout(Some(READ_TIMEOUT));
    let mut line = String::new();
    BufReader::new(Read::by_ref(&mut stream).take(MAX_REQUEST)).read_line(&mut line)?;
    let response = respond(builds, &line);
    write_line(&mut stream, &response)
}

fn write_line(stream: &mut Stream, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

/// Sends `request` to the app at `address` and returns its answer.
pub fn request(address: &Address, request: &Request) -> io::Result<Response> {
    let mut stream = Stream::connect(address.name()?)?;
    write_line(&mut stream, request)?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builds::{HangingAgent, fake_github};
    use crate::github::FakeGitHub;
    use crate::record::Kind;
    use serde_json::{Value, json};
    use std::sync::atomic::AtomicUsize;

    /// An address no other test uses.
    fn address(dir: &Path) -> Address {
        if cfg!(windows) {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::SeqCst);
            Address::Pipe(format!("epik-test-{}-{n}", std::process::id()))
        } else {
            Address::Path(dir.join("epik.sock"))
        }
    }

    /// A server on a temporary socket, serving builds under a temporary
    /// root with `github` and an agent that runs until cancelled.
    struct Running {
        root: tempfile::TempDir,
        address: Address,
        builds: Arc<Builds>,
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Running {
        fn new(github: FakeGitHub) -> Self {
            Self::in_root(tempfile::tempdir().unwrap(), github)
        }

        /// A server whose root, and socket on macOS and Linux, are in
        /// `root`.
        fn in_root(root: tempfile::TempDir, github: FakeGitHub) -> Self {
            let address = address(root.path());
            let server = Server::bind(&address).unwrap();
            let builds = Arc::new(Builds::new(
                root.path(),
                Arc::new(github),
                Arc::new(HangingAgent),
            ));
            let stop = Arc::new(AtomicBool::new(false));
            let thread = thread::spawn({
                let builds = builds.clone();
                let stop = stop.clone();
                move || server.serve(builds, &stop)
            });
            Self {
                root,
                address,
                builds,
                stop,
                thread: Some(thread),
            }
        }

        /// Sends `request`, raw, and returns the reply as JSON.
        fn send(&self, request: &str) -> Value {
            let mut stream = Stream::connect(self.address.name().unwrap()).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            stream.write_all(b"\n").unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            assert!(line.ends_with('\n'), "{line:?}");
            serde_json::from_str(&line).unwrap()
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
            self.builds.shutdown();
        }
    }

    #[test]
    fn ping_answers_with_the_version() {
        let app = Running::new(fake_github());

        let reply = app.send(r#"{"type": "ping"}"#);

        assert_eq!(reply, json!({"type": "pong", "version": crate::version()}));
    }

    #[test]
    fn build_answers_once_the_run_has_started() {
        let app = Running::new(fake_github());

        let reply = app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 7, "plugin_dir": "/plugin"}"#,
        );

        assert_eq!(reply["type"], "started", "{reply}");
        assert_eq!(reply["kind"], "issue");
        assert_eq!(reply["branch"], "main");
        let run_id = reply["run_id"].as_str().unwrap();
        assert!(run_id.starts_with("owner-repo-7-"), "{run_id}");
        let run_dir = PathBuf::from(reply["run_dir"].as_str().unwrap());
        assert_eq!(run_dir, record::run_dir(app.root.path(), run_id));
        assert!(run_dir.join(record::EVENTS_FILE).is_file());
        assert_eq!(app.builds.active(), vec![run_id.to_owned()]);
    }

    #[test]
    fn build_takes_a_branch() {
        let app = Running::new(fake_github());

        let reply = app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 7, "branch": "topic", "plugin_dir": "/plugin"}"#,
        );

        assert_eq!(reply["branch"], "topic", "{reply}");
    }

    #[test]
    fn a_build_that_cannot_start_is_an_error() {
        let app = Running::new(fake_github());

        let reply = app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 8, "plugin_dir": "/plugin"}"#,
        );

        assert_eq!(reply["type"], "error", "{reply}");
        let message = reply["message"].as_str().unwrap();
        assert!(
            message.starts_with("couldn't build owner/repo#8: "),
            "{message}"
        );
        assert!(app.builds.active().is_empty());
    }

    #[test]
    fn status_answers_with_the_runs_folded_state() {
        let app = Running::new(fake_github());
        let started = app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 7, "plugin_dir": "/plugin"}"#,
        );
        let run_id = started["run_id"].as_str().unwrap();

        let reply = app.send(&json!({"type": "status", "run_id": run_id}).to_string());

        let events = record::read_events(app.root.path(), run_id).unwrap();
        let mut expected = serde_json::to_value(record::fold(&events).unwrap()).unwrap();
        expected["type"] = json!("status");
        assert_eq!(reply, expected);
        assert_eq!(reply["status"], "running");
        let parsed: Response = serde_json::from_value(reply).unwrap();
        assert!(matches!(parsed, Response::Status(state) if state.run_id == run_id));
    }

    #[test]
    fn status_of_an_unknown_run_is_an_error() {
        let app = Running::new(fake_github());

        for run_id in ["owner-repo-9-20261028T000000Z", "../escape", ""] {
            let reply = app.send(&json!({"type": "status", "run_id": run_id}).to_string());

            assert_eq!(
                reply,
                json!({"type": "error", "message": format!("no run {run_id:?}")})
            );
        }
    }

    #[test]
    fn list_answers_with_the_runs_newest_first() {
        let app = Running::new(fake_github());
        let root = app.root.path();
        for (n, stamp) in [(1, 1), (2, 3), (3, 2)] {
            write_run(root, n, stamp);
        }

        let all = app.send(r#"{"type": "list"}"#);
        let two = app.send(r#"{"type": "list", "limit": 2}"#);

        let numbers = |reply: &Value| -> Vec<u64> {
            assert_eq!(reply["type"], "runs", "{reply}");
            let runs = reply["runs"].as_array().unwrap();
            runs.iter()
                .map(|run| run["number"].as_u64().unwrap())
                .collect()
        };
        assert_eq!(numbers(&all), [2, 3, 1]);
        assert_eq!(numbers(&two), [2, 3]);
    }

    #[test]
    fn list_returns_twenty_runs_by_default() {
        let app = Running::new(fake_github());
        for n in 1..=DEFAULT_LIMIT as u64 + 1 {
            write_run(app.root.path(), n, n as u32);
        }

        let reply = app.send(r#"{"type": "list"}"#);

        assert_eq!(reply["runs"].as_array().unwrap().len(), DEFAULT_LIMIT);
        assert_eq!(reply["runs"][0]["number"], DEFAULT_LIMIT as u64 + 1);
    }

    /// Records a run of issue `number` started `seconds` into the day.
    fn write_run(root: &Path, number: u64, seconds: u32) {
        use chrono::TimeZone;
        let at = chrono::Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap()
            + chrono::Duration::seconds(seconds.into());
        let run_id = record::run_id("owner/repo", number, at);
        let mut writer = record::RunWriter::create(root, &run_id).unwrap();
        writer
            .append(&record::Event::RunStarted {
                at,
                run_id: run_id.clone(),
                repo: "owner/repo".to_owned(),
                number,
                title: format!("Issue {number}"),
                kind: Kind::Issue,
                branch: "main".to_owned(),
                members: Vec::new(),
            })
            .unwrap();
    }

    #[test]
    fn a_bad_request_is_an_error() {
        let app = Running::new(fake_github());

        for request in [
            "not json",
            r#"{"type": "dance"}"#,
            r#"{"type": "build", "repo": "owner/repo"}"#,
            r#"{"type": "list", "limit": -1}"#,
        ] {
            let reply = app.send(request);

            assert_eq!(reply["type"], "error", "{request}: {reply}");
            let message = reply["message"].as_str().unwrap();
            assert!(message.starts_with("bad request: "), "{message}");
        }
    }

    #[test]
    fn the_client_sends_a_request_and_reads_the_reply() {
        let app = Running::new(fake_github());

        let reply = request(&app.address, &Request::Ping).unwrap();

        assert_eq!(
            reply,
            Response::Pong {
                version: crate::version().to_owned()
            }
        );
    }

    #[test]
    fn the_default_address_is_the_socket_in_the_root_or_the_pipe_epik() {
        let address = Address::in_root(Path::new("/home/me/.epik"));

        if cfg!(windows) {
            assert_eq!(address, Address::Pipe("epik".to_owned()));
        } else {
            assert_eq!(address, Address::Path("/home/me/.epik/epik.sock".into()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn only_the_user_can_open_the_socket() {
        use std::os::unix::fs::PermissionsExt;
        let app = Running::new(fake_github());
        let Address::Path(path) = &app.address else {
            unreachable!()
        };

        let mode = fs::metadata(path).unwrap().permissions().mode();

        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_socket_file_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        // A crashed app leaves its socket file behind, bound to nothing.
        let Address::Path(path) = address(root.path()) else {
            unreachable!()
        };
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let app = Running::in_root(root, fake_github());

        assert_eq!(app.send(r#"{"type": "ping"}"#)["type"], "pong");
    }

    #[test]
    fn a_socket_an_app_is_serving_is_not_taken() {
        let app = Running::new(fake_github());

        let error = Server::bind(&app.address).err().unwrap();

        // Windows refuses a second first instance of a pipe in its own way.
        if cfg!(unix) {
            assert_eq!(error.kind(), io::ErrorKind::AddrInUse, "{error}");
        }
        assert!(
            matches!(
                app.send(r#"{"type": "ping"}"#)["type"].as_str(),
                Some("pong")
            ),
            "the serving app keeps its socket"
        );
    }

    #[test]
    fn requests_and_responses_are_tagged_by_type() {
        let build = Request::Build {
            repo: "owner/repo".to_owned(),
            number: 7,
            branch: None,
            plugin_dir: "/plugin".into(),
        };
        assert_eq!(
            serde_json::to_value(&build).unwrap(),
            json!({"type": "build", "repo": "owner/repo", "number": 7, "plugin_dir": "/plugin"})
        );
        assert_eq!(
            serde_json::to_value(Request::List { limit: None }).unwrap(),
            json!({"type": "list"})
        );
        let started = Response::Started(Started {
            run_id: "r".to_owned(),
            kind: Kind::Epic,
            branch: "epic/1".to_owned(),
            run_dir: "/runs/r".into(),
        });
        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            json!({"type": "started", "run_id": "r", "kind": "epic", "branch": "epic/1", "run_dir": "/runs/r"})
        );
    }
}
