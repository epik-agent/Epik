//! Runs one implementation job in its own process.
//!
//! Protocol: a JSON [`Job`] arrives on stdin; [`Event`]s stream out as JSON
//! lines on stdout. This is the seam where a Claude Code instance will
//! eventually run. Today issues are "implemented" by appending their
//! descriptions to the output file and committing with the git CLI.

use std::fs::OpenOptions;
use std::io::{Read, Write, stdin, stdout};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use epik::implementation::{Feature, Implementable, Issue};
use epik::logging::{Event, JsonLines, Log};
use epik::repository::{Endpoint, Repository, Url};
use epik::tree::Tree;
use serde::Deserialize;

const OUTPUT_FILE: &str = "output.txt";

#[derive(Debug, Deserialize)]
struct Job {
    source: Endpoint,
    dest: Endpoint,
    issues: Tree<Issue>,
}

/// Today's issue implementation: append the description to the output file
/// and commit the result.
#[derive(Debug)]
struct AppendAndCommit(Issue);

impl Implementable for AppendAndCommit {
    fn implement(&self, _source: &Endpoint, dest: &Endpoint, log: &mut dyn Log) -> Result<()> {
        let Url(path) = dest.url();
        log.emit(Event::IssueStarted { id: self.0.id });

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.join(OUTPUT_FILE))
            .with_context(|| format!("appending to {OUTPUT_FILE}"))?;
        writeln!(file, "{}", self.0.description)?;

        let message = format!("Implement issue #{}: {}", self.0.id, self.0.description);
        git(path, &["add", OUTPUT_FILE])?;
        git(path, &["commit", "-m", &message])?;

        log.emit(Event::IssueImplemented { id: self.0.id });
        Ok(())
    }
}

/// Runs a git command in the repository, hermetically: no user or system
/// configuration, a fixed identity, and stdout kept clear of the event
/// stream.
fn git(repo: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=Epik", "-c", "user.email=epik@localhost"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::null())
        .status()
        .context("running git")?;
    ensure!(status.success(), "git {args:?} failed");
    Ok(())
}

fn main() -> Result<()> {
    let mut input = String::new();
    stdin()
        .read_to_string(&mut input)
        .context("reading job from stdin")?;
    let job: Job = serde_json::from_str(&input).context("decoding job")?;

    let feature = Feature {
        repository: Repository::new(job.dest.url().clone()),
        issues: job.issues.map(AppendAndCommit),
        reviewer: None,
    };

    let mut log = JsonLines::new(stdout().lock());
    feature.implement(&job.source, &job.dest, &mut log)
}
