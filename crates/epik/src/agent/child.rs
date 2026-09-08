//! One child process, run to completion under a deadline.
//!
//! The crate's process-spawning modules all want the same care taken:
//! both pipes drained off-thread so a chatty child cannot fill one and
//! deadlock against the wait, the outputs joined into one transcript,
//! and the child killed — and said so, in words — when the deadline
//! passes rather than held forever. It is stated once, here;
//! [`git`](crate::git) runs its binary through it and
//! the feature build's `check` runs its shell through it.

use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Spawns `command`, with no other spawn of this process in flight.
///
/// macOS makes a pipe in two steps — `pipe`, then `FD_CLOEXEC` on each
/// end — and `posix_spawn` hands a child every descriptor not yet so
/// marked. A spawn on another thread between those two steps gives
/// its child the pipe ends this one is wiring, and that child — and
/// everything it runs — holds them for as long as it lives: this one's
/// stdin never reaches EOF, its stdout never closes. One lock over
/// every spawn in the process leaves no such moment. Spawning takes
/// microseconds; nothing waits behind the lock for longer than that.
///
/// # Errors
///
/// [`Command::spawn`]'s own.
pub fn spawn(command: &mut Command) -> std::io::Result<Child> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _held = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    command.spawn()
}

/// A child that ran to its end: whether it exited zero, and its own
/// words — stdout and stderr both.
pub struct Finished {
    pub success: bool,
    pub output: String,
}

/// Runs `command` to completion within `timeout`. The pipes are wired
/// and drained here; `name` is how the child is called in the words a
/// failure comes back as.
///
/// # Errors
///
/// The child could not be started or waited on, or the deadline killed
/// it — each in words naming `name`.
pub fn run(name: &str, command: &mut Command, timeout: Duration) -> Result<Finished, String> {
    let mut child = spawn(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|error| format!("could not run {name}: {error}"))?;

    let stdout = reader(child.stdout.take().expect("stdout was piped"));
    let stderr = reader(child.stderr.take().expect("stderr was piped"));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{name} was killed after {} seconds without finishing",
                    timeout.as_secs()
                ));
            }
            Err(error) => return Err(format!("could not wait for {name}: {error}")),
        }
    };

    let mut output = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if !output.is_empty() && !stderr.is_empty() {
        output.push('\n');
    }
    output.push_str(&stderr);
    Ok(Finished {
        success: status.success(),
        output,
    })
}

/// Reads one of the child's pipes to its end, off-thread.
fn reader(pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut pipe = pipe;
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut pipe, &mut text);
        text
    })
}
