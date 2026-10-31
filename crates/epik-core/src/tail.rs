//! Agent logs, read as they grow.
//!
//! The window asks what each running agent is doing every second, and an
//! agent's log grows to megabytes. So each log is read once: [`read`]
//! remembers how far it has read every log and what it made of the lines
//! so far, and reads only the lines added since, for both
//! [`crate::activity`] and [`crate::stream`]. A line still being written
//! waits until its newline arrives. Logs no one has read for a while are
//! forgotten.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::{activity, stream};

/// How long a log no one reads is remembered.
const FORGET_AFTER: Duration = Duration::from_secs(10 * 60);

/// What the lines of a log read so far add up to.
#[derive(Default)]
pub(crate) struct Readers {
    pub(crate) activity: activity::Reader,
    pub(crate) stream: stream::Reader,
}

impl Readers {
    fn feed(&mut self, line: &str) {
        self.activity.feed(line);
        self.stream.feed(line);
    }
}

/// A log, as far as it has been read.
struct Tail {
    /// Where the next line starts.
    offset: u64,
    readers: Readers,
    read_at: Instant,
}

static TAILS: LazyLock<Mutex<HashMap<PathBuf, Tail>>> = LazyLock::new(Mutex::default);

/// What `look` makes of the log at `path`, read up to its last whole line.
/// A log shorter than what was read of it was written again, and is read
/// again from its start.
pub(crate) fn read<R>(path: &Path, look: impl FnOnce(&Readers) -> R) -> io::Result<R> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let now = Instant::now();
    let mut tails = TAILS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tails.retain(|_, tail| now.duration_since(tail.read_at) < FORGET_AFTER);
    let tail = tails.entry(path.to_owned()).or_insert_with(|| Tail {
        offset: 0,
        readers: Readers::default(),
        read_at: now,
    });
    if length < tail.offset {
        *tail = Tail {
            offset: 0,
            readers: Readers::default(),
            read_at: now,
        };
    }
    tail.read_at = now;
    file.seek(SeekFrom::Start(tail.offset))?;
    let mut added = Vec::new();
    file.read_to_end(&mut added)?;
    if let Some(end) = added.iter().rposition(|&byte| byte == b'\n') {
        for line in added[..end].split(|&byte| byte == b'\n') {
            tail.readers.feed(&String::from_utf8_lossy(line));
        }
        tail.offset += end as u64 + 1;
    }
    Ok(look(&tail.readers))
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    use serde_json::json;

    use super::*;

    fn said(text: &str) -> String {
        let update = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}});
        json!({"from_agent": {"method": "session/update", "params": {"update": update}}})
            .to_string()
    }

    fn command(id: &str, command: &str) -> String {
        let update = json!({
            "sessionUpdate": "tool_call", "toolCallId": id, "rawInput": {"command": command},
            "status": "pending", "_meta": {"claudeCode": {"toolName": "Bash"}},
        });
        json!({"from_agent": {"method": "session/update", "params": {"update": update}}})
            .to_string()
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
    }

    fn lines(lines: &[String]) -> String {
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    #[test]
    fn reading_again_reads_what_was_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.jsonl");
        let first = lines(&[said("Reading the issue."), command("t1", "cargo test")]);
        append(&path, first.as_bytes());
        assert_eq!(stream::read(&path).unwrap(), stream::parse(&first));

        let more = lines(&[said("Now the fix.")]);
        append(&path, more.as_bytes());
        let whole = format!("{first}{more}");
        assert_eq!(stream::read(&path).unwrap(), stream::parse(&whole));
        assert_eq!(
            activity::read(&path).unwrap().recent,
            activity::parse(&whole).recent
        );
    }

    #[test]
    fn a_line_still_being_written_waits_for_its_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.jsonl");
        let first = lines(&[said("Reading the issue.")]);
        append(&path, first.as_bytes());
        let next = said("Now the fix.");
        let (begun, rest) = next.split_at(next.len() / 2);
        append(&path, begun.as_bytes());
        assert_eq!(stream::read(&path).unwrap(), stream::parse(&first));

        append(&path, format!("{rest}\n").as_bytes());
        assert_eq!(
            stream::read(&path).unwrap(),
            stream::parse(&format!("{first}{next}\n"))
        );
    }

    #[test]
    fn a_character_cut_off_mid_write_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.jsonl");
        let line = said("Reading… the issue.");
        let cut = line.find('…').unwrap() + 1;
        append(&path, &line.as_bytes()[..cut]);
        assert_eq!(stream::read(&path).unwrap(), stream::parse(""));

        append(&path, &line.as_bytes()[cut..]);
        append(&path, b"\n");
        assert_eq!(stream::read(&path).unwrap(), stream::parse(&line));
    }

    #[test]
    fn a_log_written_again_is_read_again_from_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.jsonl");
        let first = lines(&[said("Reading the issue."), said("A long first log.")]);
        append(&path, first.as_bytes());
        stream::read(&path).unwrap();

        let again = lines(&[said("Again.")]);
        fs::write(&path, &again).unwrap();
        assert_eq!(stream::read(&path).unwrap(), stream::parse(&again));
    }
}
