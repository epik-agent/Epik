//! Agents: separate processes that do work and stream what they say.
//!
//! Nomenclature, project-wide: any process Epik launches to do work is
//! an *Agent*, whatever it does — a test fixture counting to three is an
//! Agent the same as a coding engine implementing an issue. An [`Agent`]
//! is that process, running from the moment [`Agent::new`] returns: it
//! says what it says as [`Event`]s, one per line, and ends in the
//! [`Exit`] that [`Agent::wait`] reports. Events flow out only. There is
//! no channel into an Agent, and it gets no stdin; what it needs to know
//! goes in its arguments, its environment, and its working directory.
//! [`ClaudeCode`](claude_code::ClaudeCode) is the first real engine, and
//! the scripted Agent under `testing` the deterministic stand-in.
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
//! threads, called pumps here, that drain both pipes continuously and hand
//! the lines to the caller as they arrive. [`Agent`] owns the process and its
//! pumps, delivers output as a stream of [`Event`]s, and kills the process if
//! it is dropped before being waited on, so nothing is left running or
//! blocked by mistake.
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
//! pumps, [`Agent::events`] is drained until the process closes its output,
//! and then [`Agent::wait`] collects the exit.
//!
//! ```no_run
//! use epik::agent::Agent;
//!
//! let mut agent = Agent::new(
//!     vec!["sh".to_string(), "-c".to_string(), "echo Hello".to_string()],
//!     "/",
//!     [],
//! )?;
//! for event in agent.events() {
//!     println!("{event:?}");
//! }
//! let exit = agent.wait()?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! The pumps keep the process from hanging whatever the caller does, so
//! calling `wait` first does not deadlock. It does mean the whole output is
//! buffered in memory until `events` is read, and that nothing is seen until
//! the process exits, which defeats the purpose of streaming.
//!
//! The types are plain and wasm-clean; the process itself rides behind
//! `native`.

use serde::{Deserialize, Serialize};

#[cfg(feature = "native")]
pub(crate) mod child;
pub mod claude_code;
#[cfg(feature = "native")]
mod platform;

#[cfg(feature = "native")]
pub use process::Agent;

/// How an Agent ended: an exit code, or the signal that took it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    /// The Agent exited on its own, with this code.
    Code(i32),
    /// A signal took the Agent.
    Signal(i32),
}

/// One line an Agent said, in the order said, without its newline — nor
/// a carriage return before it, as [`BufRead::lines`](std::io::BufRead::lines)
/// would have it. A line is decoded on its own, so bytes that are not
/// UTF-8 come through as replacement characters rather than failing the
/// stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// One line of the Agent's stdout.
    Stdout { line: String },
    /// One line of the Agent's stderr.
    Stderr { line: String },
}

#[cfg(feature = "native")]
mod process {
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread::{JoinHandle, spawn};

    use anyhow::{Context, Result, anyhow};

    use super::{Event, Exit, child, platform};
    use crate::keystore::Secret;

    /// A process whose stdout and stderr are streamed as [`Event`]s.
    ///
    /// Drain [`Agent::events`] before calling [`Agent::wait`]. The reader
    /// threads stop when the process closes its output, so the event
    /// iterator ends on its own once the process exits.
    pub struct Agent {
        event_receiver: Receiver<Event>,
        process: Child,
        pumps: Vec<JoinHandle<Result<()>>>,
    }

    impl Agent {
        /// Start `argv` — the program, then its arguments — in `cwd`, with
        /// `env` added to the environment Epik has, and begin streaming its
        /// output. The process gets no stdin.
        ///
        /// # Errors
        ///
        /// No program named, or the spawn itself failing — the program
        /// missing, chiefly. Everything after that arrives as events or
        /// through [`Agent::wait`].
        pub fn new(
            argv: Vec<String>,
            cwd: impl Into<String>,
            env: impl IntoIterator<Item = (String, Secret)>,
        ) -> Result<Self> {
            let Some((program, arguments)) = argv.split_first() else {
                return Err(anyhow!("no program to run"));
            };
            let mut command = Command::new(program);
            command.args(arguments).current_dir(cwd.into());
            for (name, value) in env {
                command.env(name, value.reveal());
            }
            let (event_sender, event_receiver) = channel();
            let (process, stdout, stderr) = spawn_piped(command)?;
            // Each pump owns one sender. When both reader threads finish
            // and drop theirs, the channel disconnects and `events` ends.
            // Nothing else may hold a sender or the iterator would never
            // terminate.
            let pumps = vec![
                pump(stdout, event_sender.clone(), |line| Event::Stdout { line }),
                pump(stderr, event_sender, |line| Event::Stderr { line }),
            ];
            Ok(Self {
                event_receiver,
                process,
                pumps,
            })
        }

        /// Events in the order they were received. Blocks between events
        /// and ends when the process has closed both stdout and stderr.
        pub fn events(&self) -> impl Iterator<Item = Event> + '_ {
            self.event_receiver.iter()
        }

        /// Wait for the reader threads to finish and the process to exit.
        ///
        /// Normally called after draining [`Agent::events`]. Calling it
        /// earlier is safe, since the channel is unbounded and the pumps
        /// keep draining the pipes, but all output is then held in memory
        /// until read. Calling this more than once returns the cached exit.
        ///
        /// # Errors
        ///
        /// A pipe could not be read to its end, or the process could not
        /// be reaped.
        pub fn wait(&mut self) -> Result<Exit> {
            for pump in self.pumps.drain(..) {
                pump.join()
                    .map_err(|_| anyhow!("reader thread panicked"))??;
            }
            let status = self.process.wait().context("could not wait for process")?;
            exit(status)
        }
    }

    impl Drop for Agent {
        /// Stop the process, and on Unix its descendants, if still running.
        ///
        /// An `Agent` dropped without [`Agent::wait`] would otherwise leave
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
            platform::kill_tree(&mut self.process);
            let _ = self.process.wait();
            platform::kill_tree(&mut self.process);
        }
    }

    /// An exit status as an [`Exit`]: the signal that took the process,
    /// else its code.
    fn exit(status: ExitStatus) -> Result<Exit> {
        #[cfg(unix)]
        if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
            return Ok(Exit::Signal(signal));
        }
        status.code().map(Exit::Code).ok_or_else(|| {
            anyhow!("the process ended neither by exit code nor by signal: {status}")
        })
    }

    /// Start `command` with stdout and stderr piped, no stdin, and on Unix
    /// as the leader of a new process group.
    ///
    /// The spawn goes through the crate's one spawn lock: macOS makes a
    /// pipe in two steps, and a spawn on another thread between them would
    /// inherit an end of these pipes and hold it open for as long as that
    /// child lives. Returns the pipes alongside the process handle so the
    /// caller can hand them to their readers without leaving `Option`s
    /// behind in `Child`.
    fn spawn_piped(mut command: Command) -> Result<(Child, ChildStdout, ChildStderr)> {
        let program = command.get_program().to_string_lossy().into_owned();
        platform::prepare(&mut command);
        let mut process = child::spawn(
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

    /// Read `process_stream` on a new thread, sending each line to
    /// `event_sender` as soon as it is complete. The thread exits when the
    /// stream reaches end of file or the receiver has been dropped.
    fn pump<R: Read + Send + 'static>(
        process_stream: R,
        event_sender: Sender<Event>,
        create_event: fn(String) -> Event,
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
                if event_sender.send(create_event(line(&buffer))).is_err() {
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
    fn line(bytes: &[u8]) -> String {
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn missing_program_is_an_error() {
            let Err(error) = Agent::new(vec!["definitely-not-a-real-program".to_owned()], "/", [])
            else {
                panic!("expected spawn to fail");
            };
            assert!(
                error.to_string().contains("could not start"),
                "unexpected error: {error:#}"
            );
        }

        #[test]
        fn no_program_is_an_error() {
            let Err(error) = Agent::new(vec![], "/", []) else {
                panic!("expected nothing to run");
            };
            assert_eq!(error.to_string(), "no program to run");
        }
    }

    /// Tests that run a real process. They depend on `sh` and the usual Unix
    /// utilities, so they are compiled only on Unix.
    #[cfg(all(test, unix))]
    mod unix_tests {
        use super::*;

        /// Everything an agent produced, one entry per line.
        struct Collected {
            stdout: Vec<String>,
            stderr: Vec<String>,
            exit: Exit,
        }

        /// `script` under `sh -c`, from `/`.
        fn sh(script: &str) -> Agent {
            Agent::new(
                vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
                "/",
                [],
            )
            .expect("could not start sh")
        }

        /// Run `script` with `sh -c` and collect its output and exit.
        fn run(script: &str) -> Collected {
            let mut agent = sh(script);
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            for event in agent.events() {
                match event {
                    Event::Stdout { line } => stdout.push(line),
                    Event::Stderr { line } => stderr.push(line),
                }
            }
            let exit = agent.wait().expect("wait failed");
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
            let mut agent = sh("head -c 1000000 /dev/zero");
            assert_eq!(agent.wait().unwrap(), Exit::Code(0));
            let total: usize = agent
                .events()
                .map(|event| match event {
                    Event::Stdout { line } | Event::Stderr { line } => line.len(),
                })
                .sum();
            assert_eq!(total, 1_000_000);
        }

        #[test]
        fn no_output_yields_no_events() {
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
            let mut agent = sh("exit 5");
            for _ in agent.events() {}
            assert_eq!(agent.wait().unwrap(), Exit::Code(5));
            assert_eq!(agent.wait().unwrap(), Exit::Code(5));
        }

        #[test]
        fn the_directory_and_environment_reach_the_process_and_stdin_is_closed() {
            let mut agent = Agent::new(
                vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    "pwd; printf '%s\\n' \"$HUSH\"; cat; echo after".to_owned(),
                ],
                "/tmp",
                [("HUSH".to_owned(), Secret::from("hush-hush-bytes"))],
            )
            .unwrap();
            let lines: Vec<String> = agent
                .events()
                .map(|event| match event {
                    Event::Stdout { line } | Event::Stderr { line } => line,
                })
                .collect();
            assert_eq!(agent.wait().unwrap(), Exit::Code(0));
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
            let agent = sh("exec sleep 30");
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
            let agent = sh("sleep 30 & echo $!; wait");
            let Some(Event::Stdout { line }) = agent.events().next() else {
                panic!("expected the grandchild pid on stdout");
            };
            let grandchild: u32 = line.trim().parse().unwrap();
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
            let mut agent = sh("true");
            for _ in agent.events() {}
            assert_eq!(agent.wait().unwrap(), Exit::Code(0));
            drop(agent);
        }
    }
}
