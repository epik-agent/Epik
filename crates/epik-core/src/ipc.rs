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

use chrono::{DateTime, Utc};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, Listener, ListenerNonblockingMode, ListenerOptions, Name,
    Stream,
};
use serde::{Deserialize, Serialize};

use crate::builds::{Builds, Started};
use crate::record;
use crate::run::{self, RunKey, RunState};
use crate::settings::Settings;
use crate::setup;

/// How many runs `list` returns when it isn't given a limit.
pub const DEFAULT_LIMIT: usize = 20;

/// The longest request read. A build request is a few hundred bytes.
const MAX_REQUEST: u64 = 64 * 1024;

/// The most `read` returns, unless the first line alone is longer.
pub const MAX_CHUNK: usize = 1024 * 1024;

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
        /// The agent and model each role runs with, or `None` for the
        /// app's settings.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roles: Option<Settings>,
    },
    /// A run's state.
    Status { run: RunKey },
    /// The runs, newest activity first.
    List {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Every run's key, with no limit.
    Runs,
    /// Every regular file in a run's record directory.
    Files { run: RunKey },
    /// The whole lines of a run's record file from a byte offset.
    Read {
        run: RunKey,
        path: String,
        offset: u64,
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
    /// Every run's key, and when it last changed.
    RunKeys {
        runs: Vec<RunLatest>,
    },
    /// A run's record files.
    Files {
        run: RunKey,
        files: Vec<RecordFile>,
    },
    /// Whole lines of a record file.
    Chunk(Chunk),
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

/// A run, and when it last changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunLatest {
    pub run: RunKey,
    pub latest_at: DateTime<Utc>,
}

/// A regular file in a run's record directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordFile {
    /// Relative to the run's directory, separated by `/`.
    pub path: String,
    /// In bytes.
    pub size: u64,
}

/// The whole lines of a record file from `offset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub path: String,
    pub offset: u64,
    /// The lines, decoded lossily where they aren't UTF-8.
    pub data: String,
    /// The offset after the lines read, from which to read next.
    pub end: u64,
    /// The file's size.
    pub size: u64,
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
            roles,
        } => match builds.start(&repo, number, branch, roles) {
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
        Request::Runs | Request::Files { .. } | Request::Read { .. } => {
            records(builds.root(), request)
        }
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

/// Answers `runs`, `files` and `read` from the records under `root`.
/// Any other request is a `bad request`.
pub(crate) fn records(root: &Path, request: Request) -> Response {
    match request {
        Request::Runs => run_keys(root),
        Request::Files { run } => files(root, &run),
        Request::Read { run, path, offset } => read(root, &run, path, offset),
        other => Response::error(format!("bad request: {other:?} is not about records")),
    }
}

fn run_keys(root: &Path) -> Response {
    let latest = || -> io::Result<Vec<RunLatest>> {
        let mut runs = Vec::new();
        for key in run::keys(root)? {
            if let Some(state) = run::fold(&record::list_builds(root, &key)?) {
                runs.push(RunLatest {
                    run: key,
                    latest_at: state.latest_at,
                });
            }
        }
        runs.sort_by(|a, b| a.run.cmp(&b.run));
        Ok(runs)
    };
    match latest() {
        Ok(runs) => Response::RunKeys { runs },
        Err(error) => Response::error(format!("couldn't list the runs: {error}")),
    }
}

/// `run`'s record directory, or `None` if it has none.
fn existing_run_dir(root: &Path, key: &RunKey) -> Option<PathBuf> {
    let dir = run::run_dir(root, key);
    dir.is_dir().then_some(dir)
}

fn files(root: &Path, key: &RunKey) -> Response {
    let Some(dir) = existing_run_dir(root, key) else {
        return Response::error(format!("no run {key}"));
    };
    let mut files = Vec::new();
    match walk(&dir, "", &mut files) {
        Ok(()) => {
            files.sort_by(|a, b| a.path.cmp(&b.path));
            Response::Files {
                run: key.clone(),
                files,
            }
        }
        Err(error) => Response::error(format!("couldn't list run {key}'s files: {error}")),
    }
}

/// Adds every regular file under `dir`, named `prefix` from the run's
/// directory, to `files`. Symbolic links aren't followed.
fn walk(dir: &Path, prefix: &str, files: &mut Vec<RecordFile>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), &format!("{path}/"), files)?;
        } else if kind.is_file() {
            files.push(RecordFile {
                path,
                size: entry.metadata()?.len(),
            });
        }
    }
    Ok(())
}

fn read(root: &Path, key: &RunKey, path: String, offset: u64) -> Response {
    let Some(dir) = existing_run_dir(root, key) else {
        return Response::error(format!("no run {key}"));
    };
    let Some(relative) = within(&path) else {
        return Response::error(format!(
            "bad request: {path:?} is not a path in the run's directory"
        ));
    };
    match read_lines(&dir.join(relative), offset) {
        Ok((data, end, size)) => Response::Chunk(Chunk {
            path,
            offset,
            data,
            end,
            size,
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Response::error(format!("run {key} has no file {path}"))
        }
        Err(error) => Response::error(format!("couldn't read {path} of run {key}: {error}")),
    }
}

/// `path` as a relative path that stays below where it is joined: not
/// empty, not absolute, and without `..`.
fn within(path: &str) -> Option<PathBuf> {
    use std::path::Component;
    let path = Path::new(path);
    let stays = path
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    (stays
        && path
            .components()
            .any(|part| matches!(part, Component::Normal(_))))
    .then(|| path.to_path_buf())
}

/// The whole lines of the file at `path` from `offset`, up to
/// [`MAX_CHUNK`] bytes unless the first line alone is longer, with the
/// offset after them and the file's size. A trailing line without its
/// newline is still being written, and is left for the next read.
fn read_lines(path: &Path, offset: u64) -> io::Result<(String, u64, u64)> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    if offset >= size {
        return Ok((String::new(), offset, size));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    Read::by_ref(&mut reader)
        .take(MAX_CHUNK as u64)
        .read_to_end(&mut bytes)?;
    match bytes.iter().rposition(|&byte| byte == b'\n') {
        Some(last) => bytes.truncate(last + 1),
        None if bytes.len() == MAX_CHUNK => {
            // The first line is longer than a chunk: finish it.
            reader.read_until(b'\n', &mut bytes)?;
            if bytes.last() != Some(&b'\n') {
                bytes.clear();
            }
        }
        None => bytes.clear(),
    }
    let end = offset + bytes.len() as u64;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), end, size))
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
    let stream = Stream::connect(address.name()?)?;
    let reply = exchange(stream, &serde_json::to_string(request)?)?;
    Ok(serde_json::from_str(&reply)?)
}

/// Relays each line of `input` to the app at `address` as it is, and
/// writes each answer to `output` as a line, flushed, until `input` ends.
/// When the app can't be reached, the answer is an error saying why. This
/// is how a client on another machine reaches the app: `epik-app stdio`,
/// run over `ssh`.
pub fn bridge(address: &Address, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let reply = match address.name().and_then(Stream::connect) {
            Err(_) => serde_json::to_string(&Response::error(format!(
                "no Epik app is running at {address} on this machine: start it with \
                 `epik-app --headless`, or start its service"
            )))?,
            Ok(stream) => match exchange(stream, &line) {
                Ok(reply) => reply,
                Err(error) => serde_json::to_string(&Response::error(format!(
                    "couldn't reach the Epik app at {address}: {error}"
                )))?,
            },
        };
        output.write_all(reply.as_bytes())?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

/// Writes `line`, a request as JSON, to `stream`, and returns the answer,
/// a line of JSON without its `\n`.
fn exchange(mut stream: Stream, line: &str) -> io::Result<String> {
    stream.write_all(line.trim_end_matches(['\r', '\n']).as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    if !reply.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the app closed the connection without answering",
        ));
    }
    reply.truncate(reply.trim_end_matches(['\r', '\n']).len());
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentProfile;
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
                |_: &AgentProfile| Arc::new(HangingAgent),
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

        let reply = app.send(r#"{"type": "build", "repo": "owner/repo", "number": 7}"#);

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

        let reply =
            app.send(r#"{"type": "build", "repo": "owner/repo", "number": 7, "branch": "topic"}"#);

        assert_eq!(reply["branch"], "topic", "{reply}");
    }

    #[test]
    fn build_takes_the_roles_to_run() {
        let app = Running::new(fake_github());

        let roles = r#""roles": {"coding": {"agent": "claude", "model": "opus"}, "review": null}"#;
        let reply = app.send(&format!(
            r#"{{"type": "build", "repo": "owner/repo", "number": 7, {roles}}}"#
        ));

        assert_eq!(reply["type"], "started", "{reply}");
        let key = RunKey::new("owner/repo", 7);
        let events =
            record::read_events(app.root.path(), &key, reply["build_id"].as_str().unwrap())
                .unwrap();
        let roles = record::fold(&events).unwrap().roles;
        assert_eq!(roles.coding.model.as_deref(), Some("opus"));
    }

    #[test]
    fn a_build_request_from_a_client_that_still_sends_a_plugin_is_answered() {
        let app = Running::new(fake_github());

        let reply = app.send(
            r#"{"type": "build", "repo": "owner/repo", "number": 7, "plugin_dir": "/plugin"}"#,
        );

        assert_eq!(reply["type"], "started", "{reply}");
    }

    #[test]
    fn a_build_that_cannot_start_is_an_error() {
        let app = Running::new(fake_github());

        let reply = app.send(r#"{"type": "build", "repo": "owner/repo", "number": 8}"#);

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
        app.send(r#"{"type": "build", "repo": "owner/repo", "number": 7}"#);

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
                roles: Default::default(),
            })
            .unwrap();
    }

    #[test]
    fn runs_answers_with_every_runs_key_past_the_list_limit() {
        let app = Running::new(fake_github());
        let count = DEFAULT_LIMIT as u64 + 5;
        for n in 1..=count {
            write_build(app.root.path(), n, n as u32);
        }

        let reply = app.send(r#"{"type": "runs"}"#);

        assert_eq!(reply["type"], "run_keys", "{reply}");
        let runs = reply["runs"].as_array().unwrap();
        assert_eq!(runs.len(), count as usize);
        let key = RunKey::new("owner/repo", 3);
        let three = runs
            .iter()
            .find(|run| run["run"] == "owner/repo#3")
            .unwrap();
        let state = run::read(app.root.path(), &key).unwrap();
        assert_eq!(
            three,
            &json!({"run": "owner/repo#3", "latest_at": state.latest_at})
        );
    }

    #[test]
    fn files_answers_with_every_file_of_a_runs_builds() {
        let app = Running::new(fake_github());
        let root = app.root.path();
        write_build(root, 7, 1);
        write_build(root, 7, 2);
        let key = RunKey::new("owner/repo", 7);
        let builds = record::list_builds(root, &key).unwrap();
        let first = &builds[0].build_id;
        fs::write(
            record::build_dir(root, &key, first).join("8.err"),
            b"oops\n",
        )
        .unwrap();

        let reply = app.send(r#"{"type": "files", "run": "owner/repo#7"}"#);

        assert_eq!(reply["type"], "files", "{reply}");
        assert_eq!(reply["run"], "owner/repo#7");
        let mut expected = Vec::new();
        for build in &builds {
            let dir = record::build_dir(root, &key, &build.build_id);
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                expected.push(json!({
                    "path": format!("{}/{}", build.build_id, entry.file_name().to_string_lossy()),
                    "size": entry.metadata().unwrap().len(),
                }));
            }
        }
        expected.sort_by_key(|file| file["path"].as_str().unwrap().to_owned());
        assert_eq!(reply["files"], json!(expected));
        assert!(
            expected
                .iter()
                .any(|file| file["path"] == format!("{first}/8.err") && file["size"] == 5)
        );
        assert_eq!(builds.len(), 2);
    }

    #[test]
    fn files_and_read_of_an_unknown_run_are_errors() {
        let app = Running::new(fake_github());

        let files = app.send(r#"{"type": "files", "run": "owner/repo#9"}"#);
        let read = app.send(
            r#"{"type": "read", "run": "owner/repo#9", "path": "b/events.jsonl", "offset": 0}"#,
        );

        assert_eq!(
            files,
            json!({"type": "error", "message": "no run owner/repo#9"})
        );
        assert_eq!(
            read,
            json!({"type": "error", "message": "no run owner/repo#9"})
        );
    }

    /// A server with run `owner/repo#7` whose file `b/log` holds `contents`.
    fn with_log(contents: &[u8]) -> Running {
        let app = Running::new(fake_github());
        let dir = run::run_dir(app.root.path(), &RunKey::new("owner/repo", 7)).join("b");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("log"), contents).unwrap();
        app
    }

    fn read_log(app: &Running, offset: u64) -> Value {
        app.send(
            &json!({"type": "read", "run": "owner/repo#7", "path": "b/log", "offset": offset})
                .to_string(),
        )
    }

    #[test]
    fn read_answers_with_the_whole_lines_from_an_offset() {
        let app = with_log(b"one\ntwo\nthree\n");

        assert_eq!(
            read_log(&app, 0),
            json!({"type": "chunk", "path": "b/log", "offset": 0, "data": "one\ntwo\nthree\n", "end": 14, "size": 14})
        );
        assert_eq!(read_log(&app, 4)["data"], "two\nthree\n");
        assert_eq!(read_log(&app, 4)["end"], 14);
        assert_eq!(read_log(&app, 14)["data"], "");
        assert_eq!(read_log(&app, 14)["end"], 14);
    }

    #[test]
    fn read_holds_back_a_trailing_partial_line() {
        let app = with_log(b"one\ntw");

        let reply = read_log(&app, 0);

        assert_eq!(reply["data"], "one\n");
        assert_eq!(reply["end"], 4);
        assert_eq!(reply["size"], 6);
        assert_eq!(read_log(&app, 4)["data"], "");
        assert_eq!(read_log(&app, 4)["end"], 4);
    }

    #[test]
    fn read_cuts_at_the_last_whole_line_under_the_chunk_size() {
        let line = format!("{}\n", "x".repeat(999));
        let lines = MAX_CHUNK / line.len() + 2;
        let app = with_log(line.repeat(lines).as_bytes());

        let first = read_log(&app, 0);
        let fitting = MAX_CHUNK / line.len() * line.len();
        assert_eq!(first["data"].as_str().unwrap().len(), fitting);
        assert_eq!(first["end"], fitting);
        let rest = read_log(&app, fitting as u64);
        assert_eq!(
            rest["data"].as_str().unwrap().len(),
            (lines * line.len()) - fitting
        );
    }

    #[test]
    fn read_returns_a_line_over_the_chunk_size_whole() {
        let long = format!("{}\n", "y".repeat(MAX_CHUNK + 10));
        let app = with_log(format!("{long}short\n").as_bytes());

        let reply = read_log(&app, 0);

        assert_eq!(reply["data"].as_str().unwrap(), long);
        assert_eq!(reply["end"], long.len());
    }

    #[test]
    fn read_decodes_bytes_that_are_not_utf8_lossily() {
        let app = with_log(b"ok\xff\n");

        let reply = read_log(&app, 0);

        assert_eq!(reply["data"], "ok\u{fffd}\n");
        assert_eq!(reply["end"], 4, "the offset counts bytes, not characters");
    }

    #[test]
    fn read_past_the_end_returns_nothing() {
        let app = with_log(b"one\n");

        let reply = read_log(&app, 100);

        assert_eq!(
            reply,
            json!({"type": "chunk", "path": "b/log", "offset": 100, "data": "", "end": 100, "size": 4})
        );
    }

    #[test]
    fn read_refuses_a_path_that_leaves_the_runs_directory() {
        let app = with_log(b"one\n");
        let outside = app.root.path().join("secret");
        fs::write(&outside, b"secret\n").unwrap();

        for path in [
            "../../../../secret",
            "b/../../8/b/log",
            outside.to_str().unwrap(),
            "",
        ] {
            let reply = app.send(
                &json!({"type": "read", "run": "owner/repo#7", "path": path, "offset": 0})
                    .to_string(),
            );

            assert_eq!(reply["type"], "error", "{path}: {reply}");
            let message = reply["message"].as_str().unwrap();
            assert!(message.starts_with("bad request: "), "{path}: {message}");
        }
    }

    #[test]
    fn a_run_that_needs_attention_is_rerun_and_deleted() {
        let app = Running::new(fake_github());
        app.send(r#"{"type": "build", "repo": "owner/repo", "number": 7}"#);
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
            roles: None,
        };
        assert_eq!(
            serde_json::to_value(&build).unwrap(),
            json!({"type": "build", "repo": "owner/repo", "number": 7})
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
