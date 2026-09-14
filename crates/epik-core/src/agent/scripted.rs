//! A scripted engine, for tests of everything around an [`Agent`]: a
//! [`Process`] whose program is `sh`, writing the files it was given
//! and saying so.
//!
//! [`Scripted`] is data — a working directory and the files to write
//! there, each by its path relative to that directory — and
//! [`start`] is where it becomes a shell script: each file's directory
//! made, its content written whole, and a line said about it. What it
//! says is its own small protocol, modelled and read in [`protocol`],
//! so a test can check both that the files landed and that every word
//! arrived.
//!
//! [`start`]: Agent::start

mod protocol;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::{Agent, Update};
use crate::process::Process;
use crate::temp;
use protocol::Event;

/// The files an engine would write, and where.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Scripted {
    pub cwd: PathBuf,
    /// Each file's content by its path relative to `cwd`, written in
    /// this order.
    pub files: BTreeMap<PathBuf, String>,
}

impl Agent for Scripted {
    /// `sh` in the working directory, running the script.
    ///
    /// # Errors
    ///
    /// A file path that is not relative, or the spawn's.
    fn start(self) -> Result<Process> {
        let script = self.script()?;
        Process::new("sh", vec!["-c".to_owned(), script], self.cwd, [], None)
    }

    fn interpret(line: &str) -> Vec<Update> {
        protocol::interpret(line)
    }
}

impl Scripted {
    /// The whole script: started, each file made and announced, and
    /// stopped — under `set -e`, so a write that fails ends the run
    /// there with a nonzero exit and no stopped line.
    fn script(&self) -> Result<String> {
        let mut lines = vec![
            "set -e".to_owned(),
            said(&Event::Started {
                session: temp::name("scripted"),
            }),
        ];
        for (path, content) in &self.files {
            if !path.is_relative() {
                bail!(
                    "{} is not relative to the working directory",
                    path.display()
                );
            }
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                lines.push(format!("mkdir -p {}", quoted(parent)));
            }
            lines.push(format!(
                "printf '%s' {} > {}",
                quoted(content),
                quoted(path)
            ));
            lines.push(said(&Event::Wrote { path: path.clone() }));
        }
        lines.push(said(&Event::Stopped {
            files: self.files.len(),
        }));
        Ok(lines.join("\n"))
    }
}

/// The shell line that says `event` on stdout.
fn said(event: &Event) -> String {
    // An Event is three plain fields; serializing one cannot fail.
    let json = serde_json::to_string(event).expect("an event serializes");
    format!("printf '%s\\n' {}", quoted(json))
}

/// A Python library with one function and the least uv needs to
/// build and test it, by path relative to a project root; see
/// fixtures/README.md. What an engine under test writes.
#[must_use]
pub fn adder() -> BTreeMap<PathBuf, String> {
    [
        (
            ".python-version",
            include_str!("scripted/fixtures/adder/.python-version"),
        ),
        (
            "pyproject.toml",
            include_str!("scripted/fixtures/adder/pyproject.toml"),
        ),
        (
            "src/adder/__init__.py",
            include_str!("scripted/fixtures/adder/src/adder/__init__.py"),
        ),
        (
            "tests/test_add.py",
            include_str!("scripted/fixtures/adder/tests/test_add.py"),
        ),
    ]
    .into_iter()
    .map(|(path, content)| (PathBuf::from(path), content.to_owned()))
    .collect()
}

/// `text` as one shell word, whatever it holds: single-quoted, with
/// each single quote inside written as `'\''`.
fn quoted(text: impl AsRef<Path>) -> String {
    let text = text.as_ref().to_string_lossy();
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::Exit;
    use crate::temp::Scratch;

    #[test]
    fn the_files_are_written_and_every_word_arrives() {
        let scratch = Scratch::new("scripted-adder");
        let files = adder();
        let mut process = Scripted {
            cwd: scratch.0.clone(),
            files: files.clone(),
        }
        .start()
        .unwrap();
        let lines: Vec<String> = process.stdout().collect();
        assert_eq!(process.wait().unwrap(), Exit::Code(0));

        for (path, content) in &files {
            assert_eq!(
                std::fs::read_to_string(scratch.0.join(path)).unwrap(),
                *content,
                "{}",
                path.display()
            );
        }

        let events: Vec<Event> = lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let [
            Event::Started { session },
            wrote @ ..,
            Event::Stopped { files: 4 },
        ] = events.as_slice()
        else {
            panic!("started, four writes, stopped: {events:?}");
        };
        assert!(session.starts_with("scripted-"), "{session}");
        let paths: Vec<&Path> = wrote
            .iter()
            .map(|event| match event {
                Event::Wrote { path } => path.as_path(),
                other => panic!("a write: {other:?}"),
            })
            .collect();
        assert_eq!(
            paths,
            files.keys().map(PathBuf::as_path).collect::<Vec<_>>(),
            "each file announced as written, in order"
        );

        let updates: Vec<Update> = lines
            .iter()
            .flat_map(|line| Scripted::interpret(line))
            .collect();
        let mut expected = vec![Update::Session {
            id: session.clone(),
            model: "scripted".to_owned(),
        }];
        expected.extend(std::iter::repeat_n(
            Update::ToolUse {
                name: "Write".to_owned(),
            },
            4,
        ));
        expected.push(Update::Result {
            ok: true,
            text: "wrote 4 files".to_owned(),
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
        });
        assert_eq!(updates, expected);
    }

    #[test]
    fn content_with_quotes_and_percent_signs_is_written_whole() {
        let scratch = Scratch::new("scripted-quoting");
        let content = "it's 100% '$HOME' \"quoted\" \\ backslash\n\nno trailing newline";
        let mut process = Scripted {
            cwd: scratch.0.clone(),
            files: [("odd 'name'.txt".into(), content.to_owned())].into(),
        }
        .start()
        .unwrap();
        for _ in process.stdout() {}
        assert_eq!(process.wait().unwrap(), Exit::Code(0));
        assert_eq!(
            std::fs::read_to_string(scratch.0.join("odd 'name'.txt")).unwrap(),
            content
        );
    }

    #[test]
    fn a_path_that_is_not_relative_is_refused_before_anything_runs() {
        let scratch = Scratch::new("scripted-absolute");
        let error = Scripted {
            cwd: scratch.0.clone(),
            files: [("/etc/motd".into(), "no".to_owned())].into(),
        }
        .start()
        .err()
        .expect("refused")
        .to_string();
        assert_eq!(error, "/etc/motd is not relative to the working directory");
    }
}
