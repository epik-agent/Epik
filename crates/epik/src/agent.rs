//! Launching Agents: separate processes that do work and stream
//! observations back.
//!
//! Nomenclature, project-wide: any process launched through the runner is
//! an *Agent*, whatever it does — a test fixture counting to three is an
//! Agent the same as a coding engine implementing an issue. An [`Agent`]
//! here is only a recipe: the [`Task`] to run. The `epik-agent` binary —
//! the runner — supervises the actual child process, and this module is
//! the single source of truth for the wire between them, the same
//! pattern as the Tauri IPC types: one vocabulary, no mirrored copies.
//! A [`Task`] goes down the runner's stdin; [`Event`]s come back up its
//! stdout as JSON lines.
//!
//! [`run`] speaks to the runner from the launcher's side: spawn it in a
//! fresh process group — which the child inherits, so one group holds
//! the whole tree and [`Handle::kill`] can end all of it at once, even
//! mid-spawn, with no pid bookkeeping to race — feed it the task, and
//! stream the decoded events into one [`Sender`]. All routing beyond
//! that is the caller's concern: clones of the `Sender` are the fan-out.
//! Events flow out only; there is no command channel into an Agent.
//!
//! The types are plain and wasm-clean; process machinery rides behind
//! `native`, and is unix-only like agent launching itself.
//!
//! [`Sender`]: std::sync::mpsc::Sender

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// Task's env values are Secrets, so the type is part of this vocabulary:
// re-exported so agent callers need nothing beyond this module.
pub use crate::keystore::Secret;

pub mod claude_code;
#[cfg(feature = "scripted")]
pub mod scripted;
#[cfg(feature = "scripted")]
pub use scripted::Scripted;

/// How a child ended: an exit code, or the signal that took it. Exactly
/// one is set.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// One observation from the runner, in the order it happened. The tag is
/// the forward-compatibility contract, like the transcript's: a newer
/// runner may say things this build has no variant for, and decoding
/// absorbs them as [`Unknown`](Self::Unknown) rather than failing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The child is running, under this pid.
    Started { pid: u32 },
    /// One line of the child's stdout.
    Stdout { line: String },
    /// One line of the child's stderr.
    Stderr { line: String },
    /// The child is gone; the run is over.
    Exited(Exit),
    /// Something a newer runner said. Tolerated, never fatal, and never
    /// serialized by this build — it exists only on the decoding side.
    #[serde(other)]
    Unknown,
}

/// What to run: the launcher's recipe, and, serialized, exactly what
/// goes down the runner's stdin. One struct on both sides of the wire —
/// the runner imports this very type — so there is no separate wire
/// shape to drift from it.
///
/// Constructing a Task touches no keystore: callers resolve secrets and
/// hand them in. Environment values stay [`Secret`] throughout, and
/// serialization is the one door their bytes leave through; the runner
/// reveals them only into the child's environment. `Debug` is safe
/// because a `Secret` redacts itself, not because anyone left the
/// derive off.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Task {
    /// The child command line; `argv[0]` is the program.
    pub argv: Vec<String>,
    /// Added to the environment the runner inherited. A map: one value
    /// per name, serialized as a JSON object in a fixed order.
    pub env: BTreeMap<String, Secret>,
    /// The child's working directory. Absolute.
    pub cwd: String,
    /// Written to the child's stdin, which is then closed; `None` gives
    /// the child no stdin at all.
    pub stdin: Option<String>,
}

/// Something that can be run as an Agent: it yields the [`Task`]. That
/// is the whole contract — no lifecycle, no conformance, nothing else.
pub trait Agent {
    fn task(&self) -> Task;
}

#[cfg(all(feature = "native", unix))]
pub use native::{Handle, run};

#[cfg(all(feature = "native", unix))]
mod native {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};

    use super::{Agent, Event, Exit};

    /// Launches `agent` under the runner at `runner` — an explicit path;
    /// no PATH divination — and streams every decoded [`Event`] into
    /// `events`.
    ///
    /// The runner goes into a fresh process group that its child
    /// inherits, so the whole tree answers to [`Handle::kill`] as one.
    /// The event stream always ends with `Exited`: the runner reports
    /// the child's own end, and a runner killed mid-run has one
    /// synthesized from the signal that took it.
    ///
    /// # Errors
    ///
    /// The spawn itself failing — the runner binary missing, chiefly.
    /// Everything after that arrives as events or through
    /// [`Handle::wait`].
    pub fn run(
        agent: &impl Agent,
        runner: &Path,
        events: Sender<Event>,
    ) -> std::io::Result<Handle> {
        use std::os::unix::process::{CommandExt, ExitStatusExt};

        let task = serde_json::to_string(&agent.task()).expect("the task serializes");
        let mut child = Command::new(runner)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        #[allow(clippy::cast_possible_wrap)]
        let group = child.id() as i32;

        let mut stdin = child.stdin.take().expect("the runner's stdin was piped");
        std::thread::spawn(move || {
            let _ = stdin.write_all(task.as_bytes());
        });

        let stdout = child.stdout.take().expect("the runner's stdout was piped");
        let mut stderr = child.stderr.take().expect("the runner's stderr was piped");
        let finished = Arc::new(AtomicBool::new(false));
        let (outcome_in, outcome) = channel();
        std::thread::spawn({
            let finished = Arc::clone(&finished);
            move || {
                let mut exit = None;
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match serde_json::from_str::<Event>(&line) {
                        // A newer runner's vocabulary, or a line that is
                        // not an event at all: not this build's business.
                        Ok(Event::Unknown) | Err(_) => {}
                        Ok(event) => {
                            if let Event::Exited(seen) = &event {
                                exit = Some(*seen);
                            }
                            let _ = events.send(event);
                        }
                    }
                }
                // Runner faults are one stderr line; read after EOF.
                let mut fault = String::new();
                let _ = stderr.read_to_string(&mut fault);
                let status = child.wait();
                finished.store(true, Ordering::SeqCst);
                let settled = match (exit, status) {
                    (Some(exit), _) => Ok(exit),
                    // A runner killed mid-run took its child with it —
                    // one process group — so the child's end is the same
                    // signal; say so on the stream, which always ends
                    // with Exited.
                    (None, Ok(status)) if status.signal().is_some() => {
                        let exit = Exit {
                            code: None,
                            signal: status.signal(),
                        };
                        let _ = events.send(Event::Exited(exit));
                        Ok(exit)
                    }
                    (None, Ok(status)) => Err(if fault.trim().is_empty() {
                        format!("the runner exited ({status}) without reporting")
                    } else {
                        fault.trim().to_owned()
                    }),
                    (None, Err(error)) => Err(format!("could not reap the runner: {error}")),
                };
                let _ = outcome_in.send(settled);
            }
        });

        Ok(Handle {
            group,
            finished,
            outcome,
        })
    }

    /// A running Agent, by the handle: kill it, or wait it out. Dropping
    /// the handle kills the whole process tree — an abandoned run must
    /// not leak one.
    #[derive(Debug)]
    pub struct Handle {
        group: i32,
        finished: Arc<AtomicBool>,
        outcome: Receiver<Result<Exit, String>>,
    }

    impl Handle {
        /// Kills the runner's process group — runner, child, and
        /// whatever the child spawned. Idempotent; the `Exited` event
        /// and the reaping still complete on the stream and in
        /// [`wait`](Self::wait). Guarded once the runner is reaped, so a
        /// recycled pid is never signalled.
        pub fn kill(&self) {
            if !self.finished.load(Ordering::SeqCst) {
                // SAFETY: killpg with a valid signal; failure (already
                // gone) is the idempotent case and is ignored.
                unsafe {
                    libc::killpg(self.group, libc::SIGKILL);
                }
            }
        }

        /// The final exit, once the run is over: the child's end, or the
        /// runner's own fault in words.
        ///
        /// # Errors
        ///
        /// The runner faulted — bad task, spawn failure — in its own one
        /// line.
        pub fn wait(self) -> Result<Exit, String> {
            self.outcome
                .recv()
                .unwrap_or_else(|_| Err("the runner's observer died unsettled".to_owned()))
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            self.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_crosses_the_wire_tagged_and_comes_back_itself() {
        for (event, tag) in [
            (Event::Started { pid: 7 }, r#""event":"started""#),
            (
                Event::Stdout {
                    line: "one".to_owned(),
                },
                r#""event":"stdout""#,
            ),
            (
                Event::Stderr {
                    line: "oops".to_owned(),
                },
                r#""event":"stderr""#,
            ),
            (
                Event::Exited(Exit {
                    code: Some(0),
                    signal: None,
                }),
                r#""event":"exited""#,
            ),
        ] {
            let wire = serde_json::to_string(&event).unwrap();
            assert!(wire.contains(tag), "{wire}");
            let received: Event = serde_json::from_str(&wire).unwrap();
            assert_eq!(received, event);
        }
    }

    /// A newer runner may say things this build has no variant for; the
    /// stream absorbs them rather than dying on them.
    #[test]
    fn an_unknown_event_from_a_future_runner_is_tolerated() {
        let received: Event =
            serde_json::from_str(r#"{"event":"teleported","where":"elsewhere"}"#).unwrap();
        assert_eq!(received, Event::Unknown);
    }

    #[test]
    fn a_tasks_debug_never_shows_a_secrets_bytes() {
        let task = Task {
            argv: vec!["sh".to_owned()],
            env: BTreeMap::from([("API_KEY".to_owned(), Secret::from("hush-hush-bytes"))]),
            cwd: "/".to_owned(),
            stdin: None,
        };
        let debugged = format!("{task:?}");
        assert!(!debugged.contains("hush-hush-bytes"), "{debugged}");
        assert!(debugged.contains("API_KEY"), "the name is not the secret");
    }

    /// Secrets stay `Secret` through the task; serializing it for the
    /// runner is the one door their bytes leave through — and they come
    /// back a `Secret` on the runner's side of the wire. The runner reads
    /// `env` as a JSON object keyed by name; that shape is pinned here.
    #[test]
    fn the_wire_to_the_runner_is_the_one_reveal() {
        let task = Task {
            argv: vec!["sh".to_owned()],
            env: BTreeMap::from([("API_KEY".to_owned(), Secret::from("hush-hush-bytes"))]),
            cwd: "/".to_owned(),
            stdin: Some("payload".to_owned()),
        };
        let wire = serde_json::to_string(&task).unwrap();
        assert!(wire.contains("hush-hush-bytes"), "the wire carries bytes");
        let shape: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert!(
            shape["env"].is_object(),
            "env is an object keyed by name: {wire}"
        );
        let read_back: Task = serde_json::from_str(&wire).unwrap();
        assert_eq!(read_back.env["API_KEY"], Secret::from("hush-hush-bytes"));
        assert_eq!(read_back.stdin.as_deref(), Some("payload"));
    }
}
