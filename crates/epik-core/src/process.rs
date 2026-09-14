//! Processes: what Epik launches to do work.
//!
//! A [`Process`] is a running process. It says what it says as lines,
//! [`stdout`](Process::stdout) and [`stderr`](Process::stderr) each its
//! own stream, and ends in the [`Exit`] that [`Process::wait`] reports.
//! Nothing flows in — no stdin,
//! no channel; what it needs to know goes in its arguments, its
//! environment, and its working directory. Given a deadline, `wait`
//! kills it when the deadline passes. Every process Epik starts is a
//! Process: a coding engine, a git command, a check.
//!
//! # Example
//!
//! [`Process::new`] starts the process, [`Process::stdout`] is drained
//! until the process closes it, and [`Process::wait`] collects the
//! exit.
//!
//! ```no_run
//! # #[cfg(feature = "native")] {
//! use epik_core::process::Process;
//!
//! let mut process = Process::new(
//!     vec!["sh".to_string(), "-c".to_string(), "echo Hello".to_string()],
//!     "/",
//!     [],
//!     None,
//! )?;
//! for line in process.stdout() {
//!     println!("{line}");
//! }
//! let exit = process.wait()?;
//! # }
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
mod platform;
mod shared;

pub use shared::Exit;

use std::process::{Child, Command};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use crate::keystore::Secret;
use shared::{exit, pump, spawn_piped};

/// A process whose stdout and stderr are streamed as lines, each
/// without its newline — nor a carriage return before it, as
/// [`BufRead::lines`](std::io::BufRead::lines) would have it — and
/// decoded on its own, so bytes that are not UTF-8 come through as
/// replacement characters rather than failing the stream.
///
/// The process is started in its own group, on Unix, and dropping the
/// `Process` kills the whole group: a helper the process left in the
/// background holds the pipes open, and would otherwise keep the pumps
/// waiting after the process itself was gone.
///
/// Drain [`Process::stdout`] and [`Process::stderr`] before calling
/// [`Process::wait`]. The reader threads stop when the process closes
/// each stream, so each iterator ends on its own once the process
/// exits. Reading one stream to its end blocks until the process
/// closes it, whatever it says on the other meanwhile; a caller that
/// follows both as they come reads each on a thread of its own, and a
/// `Process` can be shared between threads for that. A stream has one
/// reader at a time: a second call for the same stream blocks until
/// the first iterator is dropped.
pub struct Process {
    /// The program, for the words a failure comes back as.
    program: String,
    /// When the process is killed if still running, and how long it
    /// was given; `None` lets it run for as long as it likes.
    deadline: Option<(Instant, Duration)>,
    /// Each behind a lock, so a `Process` can be shared between the
    /// threads reading its two streams.
    stdout_receiver: Mutex<Receiver<String>>,
    stderr_receiver: Mutex<Receiver<String>>,
    child: Child,
    pumps: Vec<JoinHandle<Result<()>>>,
}

/// A process run to its end: how it ended, and everything it said on
/// each stream, in order, lines joined by newlines — `None` for a
/// stream it said nothing on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finished {
    pub exit: Exit,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl Process {
    /// Start `argv` — the program, then its arguments — in `cwd`, with
    /// `env` added to the environment Epik has, and begin streaming its
    /// output. The process gets no stdin. With a `deadline`, a process
    /// still running that long after its start is killed by
    /// [`Process::wait`], which says so; `None` is no limit.
    ///
    /// # Errors
    ///
    /// No program named, or the spawn itself failing — the program
    /// missing, chiefly. Everything after that arrives on the streams
    /// or through [`Process::wait`].
    pub fn new(
        argv: Vec<String>,
        cwd: impl Into<String>,
        env: impl IntoIterator<Item = (String, Secret)>,
        deadline: Option<Duration>,
    ) -> Result<Self> {
        let Some((program, arguments)) = argv.split_first() else {
            return Err(anyhow!("no program to run"));
        };
        let mut command = Command::new(program);
        command.args(arguments).current_dir(cwd.into());
        for (name, value) in env {
            command.env(name, value.reveal());
        }
        let (stdout_sender, stdout_receiver) = channel();
        let (stderr_sender, stderr_receiver) = channel();
        let (child, stdout, stderr) = spawn_piped(command)?;
        let deadline = deadline.map(|allowed| (Instant::now() + allowed, allowed));
        // Each pump owns its stream's one sender. When the reader thread
        // finishes and drops it, the channel disconnects and the
        // stream's iterator ends. Nothing else may hold a sender or the
        // iterator would never terminate.
        let pumps = vec![pump(stdout, stdout_sender), pump(stderr, stderr_sender)];
        Ok(Self {
            program: program.clone(),
            deadline,
            stdout_receiver: Mutex::new(stdout_receiver),
            stderr_receiver: Mutex::new(stderr_receiver),
            child,
            pumps,
        })
    }

    /// The lines of stdout, in order. Blocks between lines and ends
    /// when the process has closed stdout.
    pub fn stdout(&self) -> impl Iterator<Item = String> + '_ {
        lines(&self.stdout_receiver)
    }

    /// The lines of stderr, in order. Blocks between lines and ends
    /// when the process has closed stderr.
    pub fn stderr(&self) -> impl Iterator<Item = String> + '_ {
        lines(&self.stderr_receiver)
    }

    /// Wait for the reader threads to finish and the process to exit.
    ///
    /// Normally called after draining the streams. Calling it earlier
    /// is safe, since the channels are unbounded and the pumps keep
    /// draining the pipes, but all output is then held in memory until
    /// read. Calling this more than once returns the cached exit.
    ///
    /// The deadline is enforced here: a process still running when it
    /// passes is killed, with its group, and the kill is the error. A
    /// caller with a deadline waits first and reads the streams after,
    /// since draining them blocks for as long as the process holds its
    /// pipes open.
    ///
    /// # Errors
    ///
    /// The deadline passed, a pipe could not be read to its end, or
    /// the process could not be reaped.
    pub fn wait(&mut self) -> Result<Exit> {
        if let Some((deadline, allowed)) = self.deadline {
            while self
                .child
                .try_wait()
                .with_context(|| format!("could not wait for {}", self.program))?
                .is_none()
            {
                if Instant::now() >= deadline {
                    platform::kill_tree(&mut self.child);
                    let _ = self.child.wait();
                    platform::kill_tree(&mut self.child);
                    return Err(anyhow!(
                        "{} was killed after {allowed:?} without finishing",
                        self.program
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        for pump in self.pumps.drain(..) {
            pump.join()
                .map_err(|_| anyhow!("reader thread panicked"))??;
        }
        let status = self
            .child
            .wait()
            .with_context(|| format!("could not wait for {}", self.program))?;
        exit(status)
    }

    /// Run to the end and collect everything: the exit, and each
    /// stream's words as one string — the shape a caller that wants a
    /// process's words rather than its stream reads. The deadline is
    /// in force.
    ///
    /// # Errors
    ///
    /// [`Process::wait`]'s.
    pub fn finish(mut self) -> Result<Finished> {
        let exit = self.wait()?;
        let joined = |lines: Vec<String>| (!lines.is_empty()).then(|| lines.join("\n"));
        Ok(Finished {
            exit,
            stdout: joined(self.stdout().collect()),
            stderr: joined(self.stderr().collect()),
        })
    }
}

/// The lines a receiver has to give, holding its lock until the
/// iterator is dropped: they end when the pump behind it has hung up.
fn lines(receiver: &Mutex<Receiver<String>>) -> impl Iterator<Item = String> + '_ {
    let receiver = receiver.lock().unwrap_or_else(PoisonError::into_inner);
    std::iter::from_fn(move || receiver.recv().ok())
}

impl Drop for Process {
    /// Stop the process, and on Unix its descendants, if still running.
    ///
    /// A `Process` dropped without [`Process::wait`] would otherwise leave
    /// the process running and the reader threads blocked on its pipes.
    /// Killing it closes the pipes, so the threads exit on their own,
    /// and the wait reaps it. The group is killed a second time once
    /// the process is reaped: a descendant whose `fork` was in flight
    /// when the first kill landed is not yet on the group's list and
    /// survives it — measured at 38 of 40 when the kill comes 100 µs
    /// after the process's last line, which is exactly when a shell
    /// forks its next command. Errors are ignored: `Drop` cannot
    /// report them and there is nothing to retry. Every call is
    /// harmless if `wait` already succeeded.
    fn drop(&mut self) {
        platform::kill_tree(&mut self.child);
        let _ = self.child.wait();
        platform::kill_tree(&mut self.child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_an_error() {
        let Err(error) = Process::new(
            vec!["definitely-not-a-real-program".to_owned()],
            "/",
            [],
            None,
        ) else {
            panic!("expected spawn to fail");
        };
        assert!(
            error.to_string().contains("could not start"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn no_program_is_an_error() {
        let Err(error) = Process::new(vec![], "/", [], None) else {
            panic!("expected nothing to run");
        };
        assert_eq!(error.to_string(), "no program to run");
    }
}

/// Tests that run a real process. They depend on `sh` and the usual Unix
/// utilities, so they are compiled only on Unix.
#[cfg(all(test, unix))]
mod unix_tests {
    use std::process::Stdio;

    use super::*;

    /// Everything a process produced, one entry per line.
    struct Collected {
        stdout: Vec<String>,
        stderr: Vec<String>,
        exit: Exit,
    }

    /// `script` under `sh -c`, from `/`.
    fn sh(script: &str) -> Process {
        Process::new(
            vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            "/",
            [],
            None,
        )
        .expect("could not start sh")
    }

    /// Run `script` with `sh -c` and collect its output and exit.
    fn run(script: &str) -> Collected {
        let mut process = sh(script);
        let stdout = process.stdout().collect();
        let stderr = process.stderr().collect();
        let exit = process.wait().expect("wait failed");
        Collected {
            stdout,
            stderr,
            exit,
        }
    }

    #[test]
    fn stdout_is_delivered() {
        let out = run("echo Hello");
        assert_eq!(out.stdout, ["Hello"]);
        assert!(out.stderr.is_empty());
        assert_eq!(out.exit, Exit::Code(0));
    }

    #[test]
    fn stderr_is_delivered() {
        let out = run("echo Oops >&2");
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr, ["Oops"]);
    }

    #[test]
    fn streams_are_kept_separate() {
        let out = run("echo out; echo err >&2; echo out again");
        assert_eq!(out.stdout, ["out", "out again"]);
        assert_eq!(out.stderr, ["err"]);
    }

    #[test]
    fn both_streams_are_followed_at_once_from_two_threads() {
        let mut process = sh("echo out; echo err >&2; echo out again");
        let (stdout, stderr) = std::thread::scope(|scope| {
            let stdout = scope.spawn(|| process.stdout().collect::<Vec<_>>());
            let stderr = scope.spawn(|| process.stderr().collect::<Vec<_>>());
            (stdout.join().unwrap(), stderr.join().unwrap())
        });
        assert_eq!(stdout, ["out", "out again"]);
        assert_eq!(stderr, ["err"]);
        assert_eq!(process.wait().unwrap(), Exit::Code(0));
    }

    #[test]
    fn output_without_trailing_newline_is_delivered() {
        let out = run("printf 'no newline'");
        assert_eq!(out.stdout, ["no newline"]);
    }

    #[test]
    fn carriage_returns_before_newlines_are_dropped() {
        let out = run("printf 'a\\r\\n\\nb\\n'");
        assert_eq!(out.stdout, ["a", "", "b"]);
    }

    #[test]
    fn bytes_that_are_not_utf8_are_replaced() {
        let out = run("printf '\\377\\376\\000'");
        assert_eq!(out.stdout, ["\u{fffd}\u{fffd}\0"]);
    }

    #[test]
    fn a_line_longer_than_any_buffer_arrives_whole() {
        // 100_000 bytes, no newline: one line, well over any read buffer.
        let out = run("head -c 100000 /dev/zero");
        assert_eq!(out.stdout.len(), 1);
        assert_eq!(out.stdout[0].len(), 100_000);
        assert!(out.stdout[0].bytes().all(|b| b == 0));
    }

    #[test]
    fn wait_before_draining_events_still_delivers_output() {
        let mut process = sh("head -c 1000000 /dev/zero");
        assert_eq!(process.wait().unwrap(), Exit::Code(0));
        let total: usize = process.stdout().map(|line| line.len()).sum();
        assert_eq!(total, 1_000_000);
    }

    #[test]
    fn no_output_yields_no_lines() {
        let out = run("true");
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
        assert_eq!(out.exit, Exit::Code(0));
    }

    #[test]
    fn exit_code_is_reported() {
        assert_eq!(run("exit 3").exit, Exit::Code(3));
    }

    #[test]
    fn a_signal_is_reported_as_one() {
        assert_eq!(run("kill -TERM $$").exit, Exit::Signal(15));
    }

    #[test]
    fn wait_can_be_called_again() {
        let mut process = sh("exit 5");
        for _ in process.stdout() {}
        assert_eq!(process.wait().unwrap(), Exit::Code(5));
        assert_eq!(process.wait().unwrap(), Exit::Code(5));
    }

    #[test]
    fn the_directory_and_environment_reach_the_process_and_stdin_is_closed() {
        let mut process = Process::new(
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "pwd; printf '%s\\n' \"$HUSH\"; cat; echo after".to_owned(),
            ],
            "/tmp",
            [("HUSH".to_owned(), Secret::from("hush-hush-bytes"))],
            None,
        )
        .unwrap();
        let lines: Vec<String> = process.stdout().collect();
        assert_eq!(process.wait().unwrap(), Exit::Code(0));
        // macOS resolves /tmp through a symlink, so only the tail is fixed.
        assert!(lines[0].ends_with("/tmp"), "{lines:?}");
        assert_eq!(
            lines[1..],
            ["hush-hush-bytes", "after"],
            "cat saw EOF at once"
        );
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
        let process = sh("exec sleep 30");
        let pid = process.child.id();
        assert!(process_exists(pid), "sleep should be running");
        drop(process);
        assert!(!process_exists(pid), "sleep should be killed and reaped");
    }

    /// Poll until `pid` no longer exists, giving up after a few seconds.
    fn wait_for_exit(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !process_exists(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn drop_kills_background_descendants() {
        // The shell starts sleep in the background, reports its pid, and
        // then waits for it, so both are alive when the process is dropped.
        let process = sh("sleep 30 & echo $!; wait");
        let Some(line) = process.stdout().next() else {
            panic!("expected the grandchild pid on stdout");
        };
        let grandchild: u32 = line.trim().parse().unwrap();
        assert!(process_exists(grandchild), "sleep should be running");
        // Without the group kill, drop would block in `wait` until the sleep
        // ran out and the shell exited, and the grandchild would be gone by
        // the time it was checked. Timing the drop tells the two apart.
        let started = Instant::now();
        drop(process);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "drop should not wait for the background sleep to finish"
        );
        // The grandchild is reparented to init, which reaps it soon after
        // the kill, so allow a moment for that to happen.
        assert!(
            wait_for_exit(grandchild),
            "background sleep should be killed with the process group"
        );
    }

    /// `script` under `sh -c`, from `/`, with `allowed` to finish.
    fn sh_within(script: &str, allowed: Duration) -> Process {
        Process::new(
            vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            "/",
            [],
            Some(allowed),
        )
        .expect("could not start sh")
    }

    #[test]
    fn a_deadline_kills_a_process_that_will_not_finish_and_says_so() {
        let mut process = sh_within("echo started; exec sleep 30", Duration::from_millis(200));
        let pid = process.child.id();
        let started = Instant::now();
        let error = process.wait().expect_err("the deadline should kill it");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            error.to_string(),
            "sh was killed after 200ms without finishing"
        );
        assert!(!process_exists(pid), "killed and reaped");
        // What it said before the kill is still there to read.
        assert_eq!(process.stdout().collect::<Vec<_>>(), ["started"]);
    }

    #[test]
    fn a_deadline_that_is_not_reached_changes_nothing() {
        let mut process = sh_within("echo quick; exit 4", Duration::from_secs(30));
        assert_eq!(process.wait().unwrap(), Exit::Code(4));
        assert_eq!(process.stdout().collect::<Vec<_>>(), ["quick"]);
    }

    #[test]
    fn finish_collects_the_exit_and_each_streams_words() {
        let finished = sh("echo out; echo err >&2; echo out again; exit 3")
            .finish()
            .unwrap();
        assert_eq!(finished.exit, Exit::Code(3));
        assert!(!finished.exit.success());
        assert_eq!(finished.stdout.as_deref(), Some("out\nout again"));
        assert_eq!(finished.stderr.as_deref(), Some("err"));

        let quiet = sh("true").finish().unwrap();
        assert!(quiet.exit.success());
        assert_eq!(quiet.stdout, None);
        assert_eq!(quiet.stderr, None);

        let blank = sh("echo").finish().unwrap();
        assert_eq!(blank.stdout.as_deref(), Some(""), "a line, empty");
    }

    #[test]
    fn drop_after_wait_is_harmless() {
        let mut process = sh("true");
        for _ in process.stdout() {}
        assert_eq!(process.wait().unwrap(), Exit::Code(0));
        drop(process);
    }
}
