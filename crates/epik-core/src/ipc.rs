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
use crate::run::{self, RunKey, RunState};
use crate::setup;

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
    Status { run: RunKey },
    /// The runs, newest activity first.
    List {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Start a new build of a run that needs attention, as its last build
    /// was started.
    Rerun { run: RunKey },
    /// Delete a run that needs attention: its builds' clones and records.
    Delete { run: RunKey },
    /// Every setup item's state.
    Setup,
    /// Whether the app is there, and its version.
    Ping,
}

/// What the app replies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The build has started: its run has written `build_started`.
    Started(Started),
    /// A run's state, folded from its builds.
    Run(RunState),
    Runs {
        runs: Vec<RunState>,
    },
    /// Every setup item's state.
    Setup {
        items: Vec<setup::Check>,
    },
    /// The run is deleted.
    Deleted {
        run: RunKey,
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
        Request::Status { run } => status(builds.root(), &run),
        Request::List { limit } => match run::list(builds.root()) {
            Ok(mut runs) => {
                runs.truncate(limit.unwrap_or(DEFAULT_LIMIT));
                Response::Runs { runs }
            }
            Err(error) => Response::error(format!("couldn't list the runs: {error}")),
        },
        Request::Rerun { run } => match builds.rerun(&run) {
            Ok(started) => Response::Started(started),
            Err(why) => Response::error(format!("couldn't rerun {run}: {why}")),
        },
        Request::Delete { run } => match builds.delete(&run) {
            Ok(()) => Response::Deleted { run },
            Err(why) => Response::error(format!("couldn't delete {run}: {why}")),
        },
        Request::Setup => Response::Setup {
            items: builds.setup(),
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

fn status(root: &Path, key: &RunKey) -> Response {
    match run::read(root, key) {
        Ok(state) => Response::Run(state),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Response::error(format!("no run {key}"))
        }
        Err(error) => Response::error(format!("couldn't read run {key}: {error}")),
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

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::Path(path) => write!(f, "{}", path.display()),
            Address::Pipe(pipe) => write!(f, r"\\.\pipe\{pipe}"),
        }
    }
}

/// The version of the app answering `ping` at `address`, or `None` if no
/// app does.
pub fn running(address: &Address) -> Option<String> {
    match request(address, &Request::Ping) {
        Ok(Response::Pong { version }) => Some(version),
        _ => None,
    }
}

/// Why [`Server::bind`] couldn't listen.
#[derive(Debug)]
pub enum BindError {
    /// An Epik app, of this version, already answers `ping` there.
    AlreadyRunning { version: String },
    /// Listening failed for another reason.
    Io(io::Error),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindError::AlreadyRunning { version } => {
                write!(f, "an Epik app {version} is already running")
            }
            BindError::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BindError {}

impl From<io::Error> for BindError {
    fn from(error: io::Error) -> Self {
        BindError::Io(error)
    }
}

/// The app's end of the socket.
pub struct Server {
    listener: Listener,
}

impl Server {
    /// Listens at `address`, which only this user can open. When an app
    /// already answers `ping` there, the error is
    /// [`BindError::AlreadyRunning`]. A socket file left there by an app
    /// that is no longer running is removed.
    pub fn bind(address: &Address) -> Result<Self, BindError> {
        let already_running = |error: io::Error| match running(address) {
            Some(version) => BindError::AlreadyRunning { version },
            None => BindError::Io(error),
        };
        let Address::Path(path) = address else {
            return listen(address)
                .map(|listener| Self { listener })
                .map_err(already_running);
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let listener = match listen(address) {
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                match already_running(error) {
                    // Nothing answers: the file is stale.
                    BindError::Io(_) => {
                        fs::remove_file(path)?;
                        listen(address)?
                    }
                    running => return Err(running),
                }
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
    // On macOS a socket accepted from a nonblocking listener is itself
    // nonblocking, and a long reply would stop when its buffer fills.
    stream.set_nonblocking(false)?;
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
    use crate::record::{self, Kind};
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

    impl Running {
        /// Stops serving and hands back the root, for another server.
        fn take_root(mut self) -> tempfile::TempDir {
            self.stop();
            let root = tempfile::tempdir().unwrap();
            std::mem::replace(&mut self.root, root)
        }

        fn stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
            self.builds.shutdown();
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.stop();
        }
    }

    #[test]
    fn ping_answers_with_the_version() {
        let app = Running::new(fake_github());

        let reply = app.send(r#"{"type": "ping"}"#);

        assert_eq!(reply, json!({"type": "pong", "version": crate::version()}));
    }

    #[test]
    fn build_answers_once_the_build_has_started() {
        let app = Running::new(fake_github());

        let reply = app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 7, "plugin_dir": "/plugin"}"#,
        );

        assert_eq!(reply["type"], "started", "{reply}");
        assert_eq!(reply["run"], "owner/repo#7");
        assert_eq!(reply["kind"], "issue");
        assert_eq!(reply["branch"], "main");
        let key = RunKey::new("owner/repo", 7);
        let build_id = reply["build_id"].as_str().unwrap();
        let build_dir = PathBuf::from(reply["build_dir"].as_str().unwrap());
        assert_eq!(
            build_dir,
            record::build_dir(app.root.path(), &key, build_id)
        );
        assert!(build_dir.join(record::EVENTS_FILE).is_file());
        assert_eq!(app.builds.active(), vec![key]);
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
        app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 7, "plugin_dir": "/plugin"}"#,
        );

        let reply = app.send(r#"{"type": "status", "run": "owner/repo#7"}"#);

        let key = RunKey::new("owner/repo", 7);
        let mut expected = serde_json::to_value(run::read(app.root.path(), &key).unwrap()).unwrap();
        expected["type"] = json!("run");
        assert_eq!(reply, expected);
        assert_eq!(reply["disposition"], "building");
        let parsed: Response = serde_json::from_value(reply).unwrap();
        assert!(matches!(parsed, Response::Run(state) if state.run == key));
    }

    #[test]
    fn status_of_an_unknown_run_is_an_error() {
        let app = Running::new(fake_github());

        let reply = app.send(r#"{"type": "status", "run": "owner/repo#9"}"#);

        assert_eq!(
            reply,
            json!({"type": "error", "message": "no run owner/repo#9"})
        );
        for bad in ["../escape#1", "owner/repo", ""] {
            let reply = app.send(&json!({"type": "status", "run": bad}).to_string());
            let message = reply["message"].as_str().unwrap();
            assert!(message.starts_with("bad request: "), "{message}");
        }
    }

    #[test]
    fn list_answers_with_the_runs_newest_first() {
        let app = Running::new(fake_github());
        let root = app.root.path();
        for (n, stamp) in [(1, 1), (2, 3), (3, 2)] {
            write_build(root, n, stamp);
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
            write_build(app.root.path(), n, n as u32);
        }

        let reply = app.send(r#"{"type": "list"}"#);

        assert_eq!(reply["runs"].as_array().unwrap().len(), DEFAULT_LIMIT);
        assert_eq!(reply["runs"][0]["number"], DEFAULT_LIMIT as u64 + 1);
    }

    /// Records a build of issue `number` started `seconds` into the day.
    fn write_build(root: &Path, number: u64, seconds: u32) {
        use chrono::TimeZone;
        let at = chrono::Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap()
            + chrono::Duration::seconds(seconds.into());
        let key = RunKey::new("owner/repo", number);
        let mut writer = record::BuildWriter::create_new(root, &key, at).unwrap();
        writer
            .append(&record::Event::BuildStarted {
                at,
                build_id: writer.build_id().to_owned(),
                repo: "owner/repo".to_owned(),
                number,
                title: format!("Issue {number}"),
                kind: Kind::Issue,
                branch: "main".to_owned(),
                issues: Vec::new(),
                closed: Vec::new(),
                plugin_dir: None,
            })
            .unwrap();
    }

    #[test]
    fn a_run_that_needs_attention_is_rerun_and_deleted() {
        let app = Running::new(fake_github());
        app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 7, "plugin_dir": "/plugin"}"#,
        );
        let refused = app.send(r#"{"type": "rerun", "run": "owner/repo#7"}"#);
        assert_eq!(
            refused,
            json!({"type": "error", "message": "couldn't rerun owner/repo#7: a build of owner/repo#7 is running"})
        );
        app.builds.shutdown();
        let app = Running::in_root(app.take_root(), fake_github());

        let rerun = app.send(r#"{"type": "rerun", "run": "owner/repo#7"}"#);
        assert_eq!(rerun["type"], "started", "{rerun}");
        assert_eq!(rerun["run"], "owner/repo#7");
        app.builds.shutdown();
        let app = Running::in_root(app.take_root(), fake_github());
        let deleted = app.send(r#"{"type": "delete", "run": "owner/repo#7"}"#);

        assert_eq!(deleted, json!({"type": "deleted", "run": "owner/repo#7"}));
        let gone = app.send(r#"{"type": "status", "run": "owner/repo#7"}"#);
        assert_eq!(
            gone,
            json!({"type": "error", "message": "no run owner/repo#7"})
        );
    }

    #[test]
    fn setup_answers_with_every_items_state() {
        let app = Running::new(fake_github());

        let reply = app.send(r#"{"type": "setup"}"#);

        assert_eq!(
            reply,
            json!({"type": "setup", "items": []}),
            "no check in tests"
        );
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
    fn a_serving_app_is_running() {
        let app = Running::new(fake_github());

        assert_eq!(running(&app.address), Some(crate::version().to_owned()));
    }

    #[test]
    fn no_app_is_running_where_nothing_listens() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(running(&address(root.path())), None);
    }

    #[cfg(unix)]
    #[test]
    fn no_app_is_running_behind_a_stale_socket_file() {
        let root = tempfile::tempdir().unwrap();
        let Address::Path(path) = address(root.path()) else {
            unreachable!()
        };
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());

        assert_eq!(running(&Address::Path(path)), None);
    }

    #[test]
    fn an_address_shows_as_its_path_or_pipe() {
        assert_eq!(
            Address::Path("/home/me/.epik/epik.sock".into()).to_string(),
            "/home/me/.epik/epik.sock"
        );
        assert_eq!(
            Address::Pipe("epik".to_owned()).to_string(),
            r"\\.\pipe\epik"
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

        assert!(
            matches!(&error, BindError::AlreadyRunning { version } if version == crate::version()),
            "{error}"
        );
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
            run: RunKey::new("o/r", 1),
            build_id: "b".to_owned(),
            kind: Kind::Epic,
            branch: "epic/1".to_owned(),
            build_dir: "/runs/o/r/1/b".into(),
        });
        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            json!({"type": "started", "run": "o/r#1", "build_id": "b", "kind": "epic", "branch": "epic/1", "build_dir": "/runs/o/r/1/b"})
        );
    }
}
