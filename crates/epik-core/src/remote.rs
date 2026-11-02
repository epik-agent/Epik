//! Reaching a remote host's app over the user's own `ssh`.
//!
//! One `ssh` process serves a host. It runs `epik-app stdio` there, which
//! relays each request line to that machine's app and writes its reply as a
//! line. `docs/app-protocol.md` describes the protocol, and the decision
//! record `docs/design-history/2026-10-31-remote-hosts.md` why it is `ssh`.

use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::hosts;
use crate::ipc::{Request, Response};

/// How long a request waits for its reply.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// How long a stopped `ssh`'s stderr has to close. Something `ssh` started,
/// such as a `ProxyCommand`, can hold it open after `ssh` itself exits.
const STDERR_GRACE: Duration = Duration::from_secs(1);

/// The app on the host, from the remote user's home.
const REMOTE_APP: &str = ".epik/bin/epik-app";

/// Something that answers the app's requests, such as a host's app.
pub trait Client {
    /// Sends `request` and returns its reply.
    fn request(&self, request: &Request) -> io::Result<Response>;
}

impl<C: Client + ?Sized> Client for Box<C> {
    fn request(&self, request: &Request) -> io::Result<Response> {
        (**self).request(request)
    }
}

impl<C: Client + ?Sized> Client for std::sync::Arc<C> {
    fn request(&self, request: &Request) -> io::Result<Response> {
        (**self).request(request)
    }
}

/// A host's app, reached through `ssh <destination>`.
pub struct Ssh {
    destination: String,
    /// The program run as `ssh`.
    pub program: PathBuf,
    /// How long a request waits for its reply.
    pub timeout: Duration,
    /// The running `ssh`, once there is one. Held from writing a request to
    /// reading its reply, so one request is in flight at a time.
    connection: Mutex<Option<Connection>>,
}

impl Ssh {
    /// The app on `destination`, an SSH destination as the user would type
    /// it, reached with the `ssh` on the `PATH`.
    pub fn new(destination: &str) -> Self {
        Self {
            destination: destination.to_owned(),
            program: PathBuf::from("ssh"),
            timeout: TIMEOUT,
            connection: Mutex::new(None),
        }
    }

    /// Sends `request` to the host's app and returns its reply. The first
    /// request starts `ssh` and checks the app's version. If `ssh` has
    /// exited since the last request, or writing to it fails, it is started
    /// again once and the request retried.
    pub fn request(&self, request: &Request) -> io::Result<Response> {
        let line = serde_json::to_string(request)?;
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut retried = false;
        loop {
            let running = connection.as_mut().is_some_and(Connection::is_running);
            let fresh = !running;
            if fresh {
                *connection = None;
                *connection = Some(self.connect()?);
            }
            let open = connection.as_mut().expect("connected above");
            match open.exchange(&line, self.timeout) {
                Ok(reply) => return Ok(reply),
                // It went away between requests.
                Err(Failure::Closed) if !fresh && !retried => {
                    *connection = None;
                    retried = true;
                }
                Err(failure) => {
                    let open = connection.take().expect("connected above");
                    return Err(self.error(failure, open));
                }
            }
        }
    }

    /// Starts `ssh`, and checks that the app it reaches is this one's minor
    /// version.
    fn connect(&self) -> io::Result<Connection> {
        if !hosts::is_destination(&self.destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{:?} is not an SSH destination", self.destination),
            ));
        }
        let mut command = Command::new(&self.program);
        command
            .args(["-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=30"])
            .arg(&self.destination)
            .args([REMOTE_APP, "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let child = command.spawn().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("couldn't run {}: {error}", self.program.display()),
            )
        })?;
        let mut connection = Connection::new(child);
        let ping = serde_json::to_string(&Request::Ping)?;
        match connection.exchange(&ping, self.timeout) {
            Ok(Response::Pong { version }) if same_minor(&version, crate::version()) => {
                Ok(connection)
            }
            Ok(Response::Pong { version }) => Err(Mismatch {
                destination: self.destination.clone(),
                version,
            }
            .into()),
            // Most likely no app is running there, and `epik-app stdio`
            // says how to start one.
            Ok(Response::Error { message }) => {
                Err(io::Error::new(io::ErrorKind::ConnectionRefused, message))
            }
            Ok(other) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} answered ping with {other:?}", self.destination),
            )),
            Err(failure) => Err(self.error(failure, connection)),
        }
    }

    /// The error for `failure` on `connection`, which is stopped.
    fn error(&self, failure: Failure, connection: Connection) -> io::Error {
        let why = connection.stop();
        match failure {
            Failure::Closed => io::Error::new(
                io::ErrorKind::ConnectionAborted,
                format!(
                    "couldn't reach {}: {}",
                    self.destination,
                    why.as_deref().unwrap_or("the connection closed")
                ),
            ),
            Failure::TimedOut => io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{} didn't answer within {} seconds",
                    self.destination,
                    self.timeout.as_secs_f64()
                ),
            ),
            Failure::Io(error) => error,
        }
    }
}

impl Client for Ssh {
    fn request(&self, request: &Request) -> io::Result<Response> {
        Ssh::request(self, request)
    }
}

/// Why a host's app is refused: it is another minor version than this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub destination: String,
    /// The version of the host's app.
    pub version: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} runs Epik {}; install {}.x there",
            self.destination,
            self.version,
            minor(crate::version()).unwrap_or(crate::version())
        )
    }
}

impl std::error::Error for Mismatch {}

impl From<Mismatch> for io::Error {
    fn from(mismatch: Mismatch) -> Self {
        io::Error::new(io::ErrorKind::Unsupported, mismatch)
    }
}

/// The version of the host's app, when `error` is that it is another
/// minor version than this one's.
pub fn mismatched(error: &io::Error) -> Option<&str> {
    error
        .get_ref()?
        .downcast_ref::<Mismatch>()
        .map(|mismatch| mismatch.version.as_str())
}

/// Why an exchange got no reply.
enum Failure {
    /// `ssh` has exited, or closed its stdin or stdout.
    Closed,
    TimedOut,
    /// A reply that isn't a response.
    Io(io::Error),
}

/// A running `ssh`, with threads reading its stdout and stderr.
struct Connection {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Each line of stdout that is a JSON object.
    replies: Receiver<String>,
    /// The last line of stderr that isn't blank.
    last_error: Arc<Mutex<Option<String>>>,
    /// Disconnected once stderr has closed.
    stderr_open: Receiver<()>,
}

impl Connection {
    fn new(mut child: Child) -> Self {
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (send, replies) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // A login shell's startup files can print before the app
                // does: a line that isn't JSON is not a reply.
                let is_json =
                    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&line)
                        .is_ok();
                if is_json && send.send(line).is_err() {
                    break;
                }
            }
        });
        let last_error = Arc::new(Mutex::new(None));
        let (open, stderr_open) = mpsc::channel::<()>();
        thread::spawn({
            let last_error = last_error.clone();
            move || {
                let _open = open;
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    if !line.trim().is_empty() {
                        *last_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(line);
                    }
                }
            }
        });
        Self {
            child,
            stdin,
            replies,
            last_error,
            stderr_open,
        }
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Writes `line` and waits up to `timeout` for the reply.
    fn exchange(&mut self, line: &str, timeout: Duration) -> Result<Response, Failure> {
        let stdin = self.stdin.as_mut().ok_or(Failure::Closed)?;
        let written = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        if written.is_err() {
            return Err(Failure::Closed);
        }
        match self.replies.recv_timeout(timeout) {
            Ok(reply) => serde_json::from_str(&reply).map_err(|error| {
                Failure::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("not a reply: {error}: {reply}"),
                ))
            }),
            Err(RecvTimeoutError::Timeout) => Err(Failure::TimedOut),
            Err(RecvTimeoutError::Disconnected) => Err(Failure::Closed),
        }
    }

    /// Stops `ssh`, and returns the last line it wrote to stderr.
    fn stop(mut self) -> Option<String> {
        self.halt();
        let _ = self.stderr_open.recv_timeout(STDERR_GRACE);
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn halt(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.halt();
    }
}

/// `version`'s major and minor parts, `0.4` of `0.4.1`.
fn minor(version: &str) -> Option<&str> {
    let (minor, patch) = version.rsplit_once('.')?;
    (minor.contains('.') && !patch.is_empty()).then_some(minor)
}

/// Whether `a` and `b` are versions with the same major and minor parts.
fn same_minor(a: &str, b: &str) -> bool {
    matches!((minor(a), minor(b)), (Some(a), Some(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_is_the_version_without_its_patch() {
        assert_eq!(minor("0.4.1"), Some("0.4"));
        assert_eq!(minor("1.12.0"), Some("1.12"));
        assert_eq!(minor("0.4"), None);
        assert_eq!(minor(""), None);
    }

    #[test]
    fn versions_match_when_only_their_patches_differ() {
        assert!(same_minor("0.4.0", "0.4.7"));
        assert!(!same_minor("0.3.2", "0.4.0"));
        assert!(!same_minor("1.4.0", "0.4.0"));
        assert!(!same_minor("nonsense", "nonsense"));
    }

    #[test]
    fn a_destination_ssh_would_take_for_an_option_is_refused() {
        let error = Ssh::new("-oProxyCommand=evil")
            .request(&Request::Ping)
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
    }
}
