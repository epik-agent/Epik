//! Launch a process and stream its output while it runs.
//!
//! # Why this exists
//!
//! We need to launch, monitor, and control separate processes from Rust.
//! [`std::process`] covers launching and waiting, but it leaves reading the
//! process's output to the caller, and that is where things go wrong. A pipe
//! holds only a few kilobytes. Once it fills, the process blocks on its next
//! write until someone reads, and a parent that is waiting for the process to
//! exit before reading will wait forever. With stdout and stderr both piped,
//! reading one while the other fills causes the same hang.
//!
//! The piece the standard library does not provide is a pair of reader
//! threads, called pumps here, that drain both pipes continuously and hand the
//! bytes to the caller as they arrive. [`Agent`] owns the process and its
//! pumps, delivers output as a stream of [`OutputChunk`] chunks, and kills
//! the process if it is dropped before being waited on, so nothing is left
//! running or blocked by mistake.
//!
//! On Unix the process is started in its own process group, and dropping the
//! `Agent` kills the whole group. A process that forks a helper and leaves it
//! in the background hands that helper its stdout and stderr, so the pipes
//! stay open, and the pumps keep waiting, until the helper exits. Killing
//! only the direct child would leave the helper running and the pumps
//! blocked. Killing the group takes every descendant with it, unless one has
//! moved itself to a group of its own, as a daemon does. The operating
//! system specifics live in [`platform`].
//!
//! # Example
//!
//! Call the methods in this order: [`Agent::new`] starts the process and the
//! pumps, [`Agent::chunks`] is drained until the process closes its output,
//! and then [`Agent::wait`] collects the exit status.
//!
//! ```no_run
//! let mut agent = Agent::new(
//!     "sh".to_string(),
//!     vec!["-c".to_string(), "echo Hello".to_string()],
//! )?;
//! for chunk in agent.chunks() {
//!     println!("{chunk:?}");
//! }
//! let status = agent.wait()?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! The pumps keep the process from hanging whatever the caller does, so
//! calling `wait` first does not deadlock. It does mean the whole output is
//! buffered in memory until `chunks` is read, and that nothing is seen until
//! the process exits, which defeats the purpose of streaming.

mod platform;

use anyhow::{Context, Result, anyhow};
use std::io::{ErrorKind, Read};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{JoinHandle, spawn};

/// A chunk of process output, exactly as the process wrote it.
///
/// Chunks arrive as soon as they are produced and need not end at a line or
/// even a character boundary. A consumer that wants text should accumulate
/// bytes and decode at a boundary meaningful to it, such as a newline.
#[derive(Debug)]
pub enum OutputChunk {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

/// A process whose stdout and stderr are streamed as [`OutputChunk`]s.
///
/// Drain [`Agent::chunks`] before calling [`Agent::wait`]. The reader threads
/// stop when the process closes its output, so the chunk iterator ends on its
/// own once the process exits.
pub struct Agent {
    chunk_receiver: Receiver<OutputChunk>,
    process: Child,
    pumps: Vec<JoinHandle<Result<()>>>,
}

impl Agent {
    /// Start `program` with `arguments` and begin streaming its output.
    pub fn new(program: String, arguments: Vec<String>) -> Result<Self> {
        let (chunk_sender, chunk_receiver) = channel();
        let (process, stdout, stderr) = spawn_piped(&program, arguments)?;
        // Each pump owns one sender. When both reader threads finish and drop
        // theirs, the channel disconnects and `chunks` ends. Nothing else may
        // hold a sender or the iterator would never terminate.
        let pumps = vec![
            pump(stdout, chunk_sender.clone(), OutputChunk::Stdout),
            pump(stderr, chunk_sender, OutputChunk::Stderr),
        ];
        Ok(Self {
            chunk_receiver,
            process,
            pumps,
        })
    }

    /// The process id. On Unix this is also the id of the process group the
    /// process leads, so a signal sent to the negative pid reaches every
    /// descendant along with it.
    pub fn process_id(&self) -> u32 {
        self.process.id()
    }

    /// Output chunks in the order they were received. Blocks between chunks
    /// and ends when the process has closed both stdout and stderr.
    pub fn chunks(&self) -> impl Iterator<Item = OutputChunk> + '_ {
        self.chunk_receiver.iter()
    }

    /// Wait for the reader threads to finish and the process to exit.
    ///
    /// Normally called after draining [`Agent::chunks`]. Calling it earlier
    /// is safe, since the channel is unbounded and the pumps keep draining
    /// the pipes, but all output is then held in memory until read. Calling
    /// this more than once returns the cached exit status.
    pub fn wait(&mut self) -> Result<ExitStatus> {
        for pump in self.pumps.drain(..) {
            pump.join()
                .map_err(|_| anyhow!("reader thread panicked"))??;
        }
        self.process.wait().context("could not wait for process")
    }
}

impl Drop for Agent {
    /// Stop the process, and on Unix its descendants, if still running.
    ///
    /// An `Agent` dropped without [`Agent::wait`] would otherwise leave the
    /// process running and the reader threads blocked on its pipes. Killing
    /// it closes the pipes, so the threads exit on their own, and the wait
    /// reaps it. Errors are ignored: `Drop` cannot report them and there is
    /// nothing to retry. Both calls are harmless if `wait` already succeeded.
    fn drop(&mut self) {
        platform::kill_tree(&mut self.process);
        let _ = self.process.wait();
    }
}

/// Start `program` with `arguments`, with stdout and stderr piped, and on
/// Unix as the leader of a new process group.
///
/// Returns the pipes alongside the process handle so the caller can hand them
/// to their readers without leaving `Option`s behind in `Child`.
fn spawn_piped(program: &str, arguments: Vec<String>) -> Result<(Child, ChildStdout, ChildStderr)> {
    let mut command = Command::new(program);
    platform::prepare(&mut command);
    let mut process = command
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
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

/// Read `process_stream` on a new thread, sending each chunk to `chunk_sender`
/// as soon as it arrives. The thread exits when the stream reaches end of
/// file or the receiver has been dropped.
fn pump<R: Read + Send + 'static>(
    mut process_stream: R,
    chunk_sender: Sender<OutputChunk>,
    create_chunk: fn(Vec<u8>) -> OutputChunk,
) -> JoinHandle<Result<()>> {
    spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            let n = match process_stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            if chunk_sender
                .send(create_chunk(buffer[..n].to_vec()))
                .is_err()
            {
                // The receiver is gone, so nobody wants the rest.
                break;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_an_error() {
        let Err(error) = Agent::new("definitely-not-a-real-program".to_string(), vec![]) else {
            panic!("expected spawn to fail");
        };
        assert!(
            error.to_string().contains("could not start"),
            "unexpected error: {error:#}"
        );
    }
}

/// Tests that run a real process. They depend on `sh` and the usual Unix
/// utilities, so they are compiled only on Unix.
#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;

    /// Everything an agent produced, with each stream's chunks concatenated.
    struct Collected {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        chunks: usize,
        status: ExitStatus,
    }

    /// Run `script` with `sh -c` and collect its output and exit status.
    fn run(script: &str) -> Collected {
        let mut agent = Agent::new("sh".to_string(), vec!["-c".to_string(), script.to_string()])
            .expect("could not start sh");
        let (mut stdout, mut stderr, mut chunks) = (Vec::new(), Vec::new(), 0);
        for chunk in agent.chunks() {
            chunks += 1;
            match chunk {
                OutputChunk::Stdout(bytes) => stdout.extend(bytes),
                OutputChunk::Stderr(bytes) => stderr.extend(bytes),
            }
        }
        let status = agent.wait().expect("wait failed");
        Collected {
            stdout,
            stderr,
            chunks,
            status,
        }
    }

    #[test]
    fn stdout_is_delivered() {
        let out = run("echo Hello");
        assert_eq!(out.stdout, b"Hello\n");
        assert!(out.stderr.is_empty());
        assert!(out.status.success());
    }

    #[test]
    fn stderr_is_delivered() {
        let out = run("echo Oops >&2");
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr, b"Oops\n");
    }

    #[test]
    fn streams_are_kept_separate() {
        let out = run("echo out; echo err >&2; echo out again");
        assert_eq!(out.stdout, b"out\nout again\n");
        assert_eq!(out.stderr, b"err\n");
    }

    #[test]
    fn output_without_trailing_newline_is_delivered() {
        let out = run("printf 'no newline'");
        assert_eq!(out.stdout, b"no newline");
    }

    #[test]
    fn non_utf8_bytes_are_preserved() {
        let out = run("printf '\\377\\376\\000'");
        assert_eq!(out.stdout, b"\xff\xfe\x00");
    }

    #[test]
    fn output_larger_than_one_chunk_arrives_intact() {
        // 100_000 bytes is well over the 4096-byte read buffer.
        let out = run("head -c 100000 /dev/zero");
        assert_eq!(out.stdout.len(), 100_000);
        assert!(out.stdout.iter().all(|&b| b == 0));
        assert!(
            out.chunks > 1,
            "expected the output to be split into chunks"
        );
    }

    #[test]
    fn wait_before_draining_chunks_still_delivers_output() {
        let mut agent = Agent::new(
            "sh".to_string(),
            vec!["-c".to_string(), "head -c 1000000 /dev/zero".to_string()],
        )
        .unwrap();
        assert!(agent.wait().unwrap().success());
        let total: usize = agent
            .chunks()
            .map(|chunk| match chunk {
                OutputChunk::Stdout(bytes) | OutputChunk::Stderr(bytes) => bytes.len(),
            })
            .sum();
        assert_eq!(total, 1_000_000);
    }

    #[test]
    fn no_output_yields_no_chunks() {
        let out = run("true");
        assert_eq!(out.chunks, 0);
        assert!(out.status.success());
    }

    #[test]
    fn exit_status_is_reported() {
        let out = run("exit 3");
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn wait_can_be_called_again() {
        let mut agent = Agent::new(
            "sh".to_string(),
            vec!["-c".to_string(), "exit 5".to_string()],
        )
        .unwrap();
        for _ in agent.chunks() {}
        assert_eq!(agent.wait().unwrap().code(), Some(5));
        assert_eq!(agent.wait().unwrap().code(), Some(5));
    }

    /// True if a process with `pid` exists, including as a zombie.
    fn process_exists(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .expect("could not run kill")
            .success()
    }

    #[test]
    fn drop_kills_and_reaps_the_process() {
        let agent = Agent::new("sleep".to_string(), vec!["30".to_string()]).unwrap();
        let pid = agent.process.id();
        assert!(process_exists(pid), "sleep should be running");
        drop(agent);
        assert!(!process_exists(pid), "sleep should be killed and reaped");
    }

    /// Poll until `pid` no longer exists, giving up after a few seconds.
    fn wait_for_exit(pid: u32) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if !process_exists(pid) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn drop_kills_background_descendants() {
        // The shell starts sleep in the background, reports its pid, and
        // then waits for it, so both are alive when the agent is dropped.
        let agent = Agent::new(
            "sh".to_string(),
            vec!["-c".to_string(), "sleep 30 & echo $!; wait".to_string()],
        )
        .unwrap();
        let Some(OutputChunk::Stdout(bytes)) = agent.chunks().next() else {
            panic!("expected the grandchild pid on stdout");
        };
        let grandchild: u32 = String::from_utf8(bytes).unwrap().trim().parse().unwrap();
        assert!(process_exists(grandchild), "sleep should be running");
        // Without the group kill, drop would block in `wait` until the sleep
        // ran out and the shell exited, and the grandchild would be gone by
        // the time it was checked. Timing the drop tells the two apart.
        let started = std::time::Instant::now();
        drop(agent);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "drop should not wait for the background sleep to finish"
        );
        // The grandchild is reparented to init, which reaps it soon after
        // the kill, so allow a moment for that to happen.
        assert!(
            wait_for_exit(grandchild),
            "background sleep should be killed with the process group"
        );
    }

    #[test]
    fn drop_after_wait_is_harmless() {
        let mut agent = Agent::new("true".to_string(), vec![]).unwrap();
        for _ in agent.chunks() {}
        assert!(agent.wait().unwrap().success());
        drop(agent);
    }
}
