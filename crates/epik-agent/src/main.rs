//! The runner: launches one command line as a supervised child and
//! frames what it observes.
//!
//! One JSON [`Spec`] arrives on stdin; the [`Event`] vocabulary leaves
//! on stdout as JSON lines — both defined once, in `epik`'s agent
//! module, which is all this binary imports from that crate. By
//! discipline it knows nothing else: no chat, no GitHub, no keystore, no
//! domain types. The runner is generic; anything launched through it is
//! an Agent.
//!
//! Lines are the framing unit. A child emitting non-UTF-8 is decoded
//! lossily; an absurdly long line is capped, with the truncation noted
//! in the line itself — nothing a child says can wedge the runner. The
//! runner's own stderr is reserved for its own faults — a bad spec, a
//! spawn failure — one human-readable line and a nonzero exit. The
//! runner exits when the child does, after saying so.
//!
//! Unix-only, like agent launching itself. The launcher starts this
//! binary in a fresh process group, which the child inherits — one group
//! for the whole tree, so a kill of that group can never leave the child
//! behind a dead runner.

#[cfg(unix)]
fn main() {
    if let Err(fault) = unix::supervise() {
        eprintln!("{fault}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("the epik agent runner is unix-only");
    std::process::exit(1);
}

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{Sender, channel};

    use epik::agent::{Event, Exit, Spec};

    /// The most of one line the runner will carry. Generous — and a
    /// child that rambles past it gets the excess dropped and the drop
    /// noted, never a wedged runner.
    const LINE_CAP: usize = 64 * 1024;

    pub(crate) fn supervise() -> Result<(), String> {
        let mut spec_json = String::new();
        std::io::stdin()
            .read_to_string(&mut spec_json)
            .map_err(|error| format!("could not read the spec from stdin: {error}"))?;
        let spec: Spec = serde_json::from_str(&spec_json)
            .map_err(|error| format!("the spec is not valid JSON: {error}"))?;
        let program = spec.argv.first().ok_or("the spec's argv is empty")?;

        let mut child = Command::new(program)
            .args(&spec.argv[1..])
            // The one reveal: out of the Secret, straight into the
            // child's environment.
            .envs(spec.env.iter().map(|(name, value)| (name, value.reveal())))
            .current_dir(&spec.cwd)
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not start {program}: {error}"))?;
        emit(&Event::Started { pid: child.id() });

        if let Some(payload) = spec.stdin {
            let mut stdin = child.stdin.take().expect("the child's stdin was piped");
            // Written off-thread: a child that never reads must not
            // deadlock the runner. Dropping the handle closes the pipe.
            std::thread::spawn(move || {
                let _ = stdin.write_all(payload.as_bytes());
            });
        }

        let (events_in, events) = channel();
        pump(
            child.stdout.take().expect("the child's stdout was piped"),
            stdout_line,
            events_in.clone(),
        );
        pump(
            child.stderr.take().expect("the child's stderr was piped"),
            stderr_line,
            events_in,
        );
        // The channel closes when both pumps finish — both pipes at EOF.
        for event in events {
            emit(&event);
        }

        let status = child
            .wait()
            .map_err(|error| format!("could not reap the child: {error}"))?;
        emit(&Event::Exited(exit_of(status)));
        Ok(())
    }

    fn exit_of(status: std::process::ExitStatus) -> Exit {
        use std::os::unix::process::ExitStatusExt;
        Exit {
            code: status.code(),
            signal: status.signal(),
        }
    }

    fn stdout_line(line: String) -> Event {
        Event::Stdout { line }
    }

    fn stderr_line(line: String) -> Event {
        Event::Stderr { line }
    }

    fn emit(event: &Event) {
        // Rust's stdout is line-buffered: each event leaves as it is
        // said.
        println!(
            "{}",
            serde_json::to_string(event).expect("events serialize")
        );
    }

    /// Drains one of the child's pipes, a capped line at a time, into
    /// the event channel.
    fn pump(pipe: impl Read + Send + 'static, frame: fn(String) -> Event, events: Sender<Event>) {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            while let Some(line) = next_line(&mut reader) {
                if events.send(frame(line)).is_err() {
                    return;
                }
            }
        });
    }

    /// The next line of `reader`: bytes to the newline, lossily decoded,
    /// capped at [`LINE_CAP`] with the truncation noted. `None` at EOF;
    /// a final unterminated line still comes back first.
    fn next_line(reader: &mut impl BufRead) -> Option<String> {
        let mut bytes = Vec::new();
        let mut dropped: usize = 0;
        while let Ok(buffer) = reader.fill_buf() {
            if buffer.is_empty() {
                if bytes.is_empty() && dropped == 0 {
                    return None;
                }
                break;
            }
            let (chunk, ended) = match buffer.iter().position(|&byte| byte == b'\n') {
                Some(at) => (&buffer[..at], true),
                None => (buffer, false),
            };
            let kept = chunk.len().min(LINE_CAP.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&chunk[..kept]);
            dropped += chunk.len() - kept;
            let consumed = chunk.len() + usize::from(ended);
            reader.consume(consumed);
            if ended {
                break;
            }
        }
        let mut line = String::from_utf8_lossy(&bytes).into_owned();
        if dropped > 0 {
            line.push_str(&format!(" … ({dropped} more bytes truncated)"));
        }
        Some(line)
    }
}
