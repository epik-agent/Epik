//! How a process ends, and the mechanics beneath a
//! [`Process`](super::Process): the spawn, one at a time, into its own
//! group; the pumps that drain its pipes; and the reading of its lines
//! and its exit.

use serde::{Deserialize, Serialize};

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, PoisonError};
use std::thread::{JoinHandle, spawn};

use anyhow::{Context, Result, anyhow};

use super::platform;

/// How a Process ended: an exit code, or the signal that took it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    /// The Process exited on its own, with this code.
    Code(i32),
    /// A signal took the Process.
    Signal(i32),
}

impl Exit {
    /// Exit code zero: the one end a process calls success.
    #[must_use]
    pub fn success(self) -> bool {
        self == Self::Code(0)
    }
}

/// An exit status as an [`Exit`]: the signal that took the process,
/// else its code.
pub(super) fn exit(status: ExitStatus) -> Result<Exit> {
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Ok(Exit::Signal(signal));
    }
    status
        .code()
        .map(Exit::Code)
        .ok_or_else(|| anyhow!("the process ended neither by exit code nor by signal: {status}"))
}

/// Start `command` with stdout and stderr piped, no stdin, and on Unix
/// as the leader of a new process group.
///
/// Returns the pipes alongside the process handle so the caller can
/// hand them to their readers without leaving `Option`s behind in
/// `Child`.
pub(super) fn spawn_piped(mut command: Command) -> Result<(Child, ChildStdout, ChildStderr)> {
    let program = command.get_program().to_string_lossy().into_owned();
    platform::prepare(&mut command);
    let mut process = spawn_one_at_a_time(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .with_context(|| format!("could not start {program}"))?;
    let stdout = process
        .stdout
        .take()
        .with_context(|| format!("{program} stdout not piped"))?;
    let stderr = process
        .stderr
        .take()
        .with_context(|| format!("{program} stderr not piped"))?;
    Ok((process, stdout, stderr))
}

/// Spawns `command`, with no other spawn of this process in flight.
///
/// macOS makes a pipe in two steps — `pipe`, then `FD_CLOEXEC` on each
/// end — and `posix_spawn` hands a child every descriptor not yet so
/// marked. A spawn on another thread between those two steps gives
/// its child the pipe ends this one is wiring, and that child — and
/// everything it runs — holds them for as long as it lives: this one's
/// stdout never closes. Measured: 4 of 3000 spawns under concurrent
/// spawn pressure saw a sibling's EOF delayed by that child's whole
/// lifetime, none with the lock. Spawning takes microseconds; nothing
/// waits behind the lock for longer than that. Every process Epik
/// starts comes through here, since every one is a Process.
fn spawn_one_at_a_time(command: &mut Command) -> std::io::Result<Child> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _held = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    command.spawn()
}

/// Read `process_stream` on a new thread, sending each line, decoded,
/// to `line_sender` as soon as it is complete. The thread exits when
/// the stream reaches end of file or the receiver has been dropped.
///
/// This is the piece [`std::process`] leaves to the caller. A pipe
/// holds a few kilobytes; a process that fills one blocks on its next
/// write until someone reads, and a parent waiting for the exit before
/// reading waits forever — with two pipes, reading one while the other
/// fills hangs the same way. A pump per pipe drains both for as long as
/// the process holds them open, whatever the caller does meanwhile.
pub(super) fn pump<R: Read + Send + 'static>(
    process_stream: R,
    line_sender: Sender<String>,
) -> JoinHandle<Result<()>> {
    spawn(move || {
        let mut reader = BufReader::new(process_stream);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return Err(e.into()),
            }
            if line_sender.send(decode(&buffer)).is_err() {
                // The receiver is gone, so nobody wants the rest.
                break;
            }
        }
        Ok(())
    })
}

/// One line's bytes as text: the newline, and a carriage return before
/// it, dropped as [`BufRead::lines`] does; anything that is not UTF-8
/// replaced.
pub(super) fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}
