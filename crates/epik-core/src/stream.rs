//! An agent's log as people read it.
//!
//! The run page shows what an agent is doing as it does it: what it says,
//! the commands it runs and what they print, the files it reads, searches,
//! edits and writes. [`parse`] turns the agent's `<issue>.jsonl`, the ACP
//! messages [`crate::agent`] records, into those [`StreamEntry`]s, in
//! order, with long text cut down to fit. Paths are named as
//! [`crate::activity`] names them, relative to where the agent works.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use chrono::DateTime;
use epik_view::{Change, Entry, Excerpt, LineRange, Stream, StreamEntry};
use serde_json::Value;

use crate::activity::{self, Call};

/// How many lines of output or of a file are shown whole. Longer text
/// keeps its first and last lines, with "… N more lines" between them, and
/// takes as many rows as this.
pub const LIMIT: usize = 12;
/// How many first lines long text keeps.
const HEAD: usize = LIMIT / 2;
/// How many last lines long text keeps, leaving a row for "… N more lines".
const TAIL: usize = LIMIT - HEAD - 1;

/// The stream in the agent log at `path`, which is read only as far as it
/// has grown since it was last read.
pub fn read(path: &Path) -> io::Result<Stream> {
    crate::tail::read(path, |readers| readers.stream.stream())
}

/// What the stream holds, in order, as far as the log has been read.
enum Item {
    Said {
        at: Option<i64>,
        text: String,
        /// The message it is part of, if the agent said.
        message: Option<String>,
    },
    Called {
        at: Option<i64>,
        id: String,
    },
}

/// A tool call, as its updates arrive.
#[derive(Default)]
struct Tracked {
    call: Call,
    /// What Claude Code says the tool did, from its latest update.
    response: Option<Value>,
}

/// An agent log's stream, read a line at a time.
#[derive(Default)]
pub struct Reader {
    dirs: Vec<String>,
    calls: HashMap<String, Tracked>,
    items: Vec<Item>,
    last_call: Option<String>,
    /// Whether something other than narration came since the last chunk,
    /// so the next chunk starts new narration.
    broken: bool,
}

/// The stream in an agent log's text. Lines that aren't JSON are skipped.
pub fn parse(text: &str) -> Stream {
    let mut reader = Reader::default();
    for line in text.lines() {
        reader.feed(line);
    }
    reader.stream()
}

impl Reader {
    /// Reads the log's next line. A line that isn't JSON is skipped.
    pub fn feed(&mut self, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let at = message
            .get("at")
            .and_then(Value::as_str)
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.timestamp_millis());
        if let Some(cwd) = message
            .pointer("/to_agent/params/cwd")
            .and_then(Value::as_str)
        {
            self.dirs.push(cwd.to_owned());
            return;
        }
        let Some(update) = message.pointer("/from_agent/params/update") else {
            return;
        };
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                let Some(chunk) = update.pointer("/content/text").and_then(Value::as_str) else {
                    return;
                };
                let id = update.get("messageId").and_then(Value::as_str);
                match self.items.last_mut() {
                    Some(Item::Said { text, message, .. })
                        if !self.broken && message.as_deref() == id =>
                    {
                        text.push_str(chunk);
                    }
                    _ => self.items.push(Item::Said {
                        at,
                        text: chunk.to_owned(),
                        message: id.map(str::to_owned),
                    }),
                }
                self.broken = false;
            }
            Some("agent_thought_chunk") => self.broken = true,
            Some("tool_call" | "tool_call_update") => {
                let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
                    return;
                };
                let items = &mut self.items;
                let last_call = &mut self.last_call;
                let tracked = self.calls.entry(id.to_owned()).or_insert_with(|| {
                    items.push(Item::Called {
                        at,
                        id: id.to_owned(),
                    });
                    *last_call = Some(id.to_owned());
                    Tracked::default()
                });
                tracked.call.apply(update);
                if activity::has_input(update) {
                    self.dirs.extend(tracked.call.dirs());
                }
                if let Some(response) = update.pointer("/_meta/claudeCode/toolResponse") {
                    tracked.response = Some(response.clone());
                }
            }
            _ => {}
        }
    }

    /// The stream so far. Its entries are made now, once every directory
    /// the agent works in so far is known, as `activity` describes its
    /// actions.
    pub fn stream(&self) -> Stream {
        let entries = self
            .items
            .iter()
            .map(|item| match item {
                Item::Said { at, text, .. } => StreamEntry {
                    at: *at,
                    what: Entry::Narration {
                        text: text.trim().to_owned(),
                    },
                },
                Item::Called { at, id } => StreamEntry {
                    at: *at,
                    what: entry(&self.calls[id], &self.dirs),
                },
            })
            .collect();
        Stream {
            entries,
            working: self
                .last_call
                .as_ref()
                .is_some_and(|id| !self.calls[id].call.finished),
        }
    }
}

/// A tool call as an entry, or in words if it isn't one the stream shows
/// in full or its input isn't known yet.
fn entry(tracked: &Tracked, dirs: &[String]) -> Entry {
    if tracked.call.tool.is_none() {
        return acp_entry(&tracked.call, dirs);
    }
    let input = tracked.call.input.as_ref();
    let response = tracked.response.as_ref();
    let field = |name: &str| input.and_then(|input| input.get(name));
    let text = |name: &str| field(name).and_then(Value::as_str);
    let number = |name: &str| field(name).and_then(Value::as_u64);
    let path = text("file_path").map(|path| activity::relative(path, dirs));
    let entry = match (tracked.call.tool.as_deref(), path) {
        (Some("Bash"), _) => tracked.call.command().map(|command| Entry::Command {
            command: command.to_owned(),
            output: cut(lines(&tracked.call.output)),
        }),
        (Some("Read"), Some(path)) => Some(Entry::Read {
            path,
            lines: match (number("offset"), number("limit")) {
                (None, None) => None,
                (from, limit) => {
                    let from = from.unwrap_or(1);
                    Some(LineRange {
                        from,
                        to: limit.map(|limit| from + limit.saturating_sub(1)),
                    })
                }
            },
        }),
        (Some(tool @ ("Grep" | "Glob")), _) => text("pattern").map(|pattern| {
            let mode = if tool == "Glob" {
                None
            } else {
                text("output_mode")
            };
            let (matches, hits) = hits(&tracked.call.output, mode, dirs);
            Entry::Search {
                pattern: pattern.to_owned(),
                within: text("path")
                    .map(|path| activity::relative(path, dirs))
                    .or_else(|| text("glob").map(str::to_owned)),
                matches,
                hits: cut(hits),
            }
        }),
        (Some("Edit" | "MultiEdit"), Some(path)) => Some(Entry::Edit {
            path,
            changes: cut(changes(input, response)),
        }),
        (Some("Write"), Some(path)) => Some(Entry::NewFile {
            path,
            content: cut(lines(
                text("content")
                    .or_else(|| response?.get("content")?.as_str())
                    .unwrap_or_default(),
            )),
        }),
        _ => None,
    };
    entry.unwrap_or_else(|| Entry::Other {
        text: activity::describe(&tracked.call, dirs),
    })
}

/// A call from an agent other than Claude Code as an entry, from ACP's
/// fields: a command and its output, a file's changes or its first lines,
/// a file read; else in words.
fn acp_entry(call: &Call, dirs: &[String]) -> Entry {
    let entry = match call.kind.as_deref() {
        Some("execute") => call.command().map(|command| Entry::Command {
            command: command.to_owned(),
            output: cut(lines(&call.output)),
        }),
        Some("edit") => call.diffs.first().map(|first| {
            let path = activity::relative(&first.path, dirs);
            let diffs = call.diffs.iter().filter(|diff| diff.path == first.path);
            if call.diffs.iter().all(|diff| diff.old.is_none()) {
                Entry::NewFile {
                    path,
                    content: cut(lines(&first.new)),
                }
            } else {
                Entry::Edit {
                    path,
                    changes: cut(diffs
                        .flat_map(|diff| {
                            difference(diff.old.as_deref().unwrap_or_default(), &diff.new)
                        })
                        .collect()),
                }
            }
        }),
        Some("read") => call.locations.first().map(|path| Entry::Read {
            path: activity::relative(path, dirs),
            lines: None,
        }),
        _ => None,
    };
    entry.unwrap_or_else(|| Entry::Other {
        text: activity::describe(call, dirs),
    })
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// How many matches a search's output reports, and where they are:
/// `file:line` for content searches, else files.
fn hits(output: &str, mode: Option<&str>, dirs: &[String]) -> (usize, Vec<String>) {
    let mut matches = 0;
    let mut hits = Vec::new();
    for line in output.lines() {
        // Its summary lines: "Found 2 files", "No matches found",
        // "(Results are truncated …)".
        if line.is_empty()
            || line.starts_with("Found ")
            || line.starts_with("No ")
            || line.starts_with('(')
        {
            continue;
        }
        match mode {
            Some("content") => {
                // `path:line:text`, or `path:text` without line numbers.
                // Context lines and separators have no `:` after the path.
                let mut parts = line.splitn(3, ':');
                let (Some(path), Some(second)) = (parts.next(), parts.next()) else {
                    continue;
                };
                let path = activity::relative(path, dirs);
                hits.push(match second.parse::<u64>() {
                    Ok(number) if parts.next().is_some() => format!("{path}:{number}"),
                    _ => path,
                });
                matches += 1;
            }
            Some("count") => {
                let Some((path, count)) = line.rsplit_once(':') else {
                    continue;
                };
                hits.push(activity::relative(path, dirs));
                matches += count.trim().parse::<usize>().unwrap_or(0);
            }
            _ => {
                hits.push(activity::relative(line, dirs));
                matches += 1;
            }
        }
    }
    (matches, hits)
}

/// The lines an edit removed and added: from Claude Code's patch if it
/// sent one, else from the text the edit replaced.
fn changes(input: Option<&Value>, response: Option<&Value>) -> Vec<Change> {
    if let Some(hunks) = response
        .and_then(|response| response.get("structuredPatch"))
        .and_then(Value::as_array)
    {
        return hunks
            .iter()
            .filter_map(|hunk| hunk.get("lines").and_then(Value::as_array))
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|line| match line.split_at_checked(1) {
                Some(("-", text)) => Some(Change::Removed(text.to_owned())),
                Some(("+", text)) => Some(Change::Added(text.to_owned())),
                _ => None,
            })
            .collect();
    }
    let edits: Vec<(&str, &str)> = match input.and_then(|input| input.get("edits")) {
        Some(Value::Array(edits)) => edits
            .iter()
            .filter_map(|edit| pair(edit, "old_string", "new_string"))
            .collect(),
        _ => input
            .and_then(|input| pair(input, "old_string", "new_string"))
            .or_else(|| response.and_then(|response| pair(response, "oldString", "newString")))
            .into_iter()
            .collect(),
    };
    edits
        .into_iter()
        .flat_map(|(old, new)| difference(old, new))
        .collect()
}

/// The text an edit replaced, and what it replaced it with, from the
/// fields named `old` and `new`.
fn pair<'a>(edit: &'a Value, old: &str, new: &str) -> Option<(&'a str, &'a str)> {
    Some((edit.get(old)?.as_str()?, edit.get(new)?.as_str()?))
}

/// The lines `old` and `new` differ in, without those they start and end
/// with in common.
fn difference(old: &str, new: &str) -> Vec<Change> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let removed = old[start..old.len() - end]
        .iter()
        .map(|line| Change::Removed(line.to_string()));
    let added = new[start..new.len() - end]
        .iter()
        .map(|line| Change::Added(line.to_string()));
    removed.chain(added).collect()
}

/// `lines` cut down to [`LIMIT`].
fn cut<T>(mut lines: Vec<T>) -> Excerpt<T> {
    if lines.len() <= LIMIT {
        return Excerpt {
            head: lines,
            ..Excerpt::default()
        };
    }
    let tail = lines.split_off(lines.len() - TAIL);
    let more = lines.len() - HEAD;
    lines.truncate(HEAD);
    Excerpt {
        head: lines,
        more,
        tail,
    }
}

#[cfg(test)]
mod tests;
