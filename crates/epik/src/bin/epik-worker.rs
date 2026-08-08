//! The worker: Epik's runs, as a local process.
//!
//! A thin main over the library, as the coding-agents ADR promises: read
//! config, resolve keys, preflight, conduct, pump. One run per invocation —
//! `--feature <n>` drains a feature's sub-issue graph, `--issue <n>`
//! conducts one issue — against `--target <branch>`, the agent on its own
//! thread while this one pumps every event to stdout as JSON lines:
//! `FeatureEvent`s for a feature, `RunEvent`s for an issue, through the
//! same `Log<E>` machinery every host pumps.
//!
//! Before any run, the preflight resolves the config-derived manifest
//! ([`epik::preflight`]) and refuses with rendered `CapabilityStatus`es —
//! nothing on stdout, no backtrace, no worktree. Boot-integrity failures
//! (unparseable config, an unwritable Epik home) are the only
//! fatal-at-startup class, and the exit code tells every class apart:
//!
//! - 0 — the run's verdict is done
//! - 1 — the run failed: the verdict's report, or a run that could not be
//!   provisioned
//! - 2 — the command line is malformed
//! - 3 — a capability refusal: one rendered `CapabilityStatus` per stderr
//!   line
//! - 4 — a boot-integrity failure

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    worker::main()
}

// ClaudeCode is unix-only by decision — its governance is the process-group
// kill — and a worker with no governable agent is no worker.
#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("epik-worker runs only on unix");
    std::process::ExitCode::FAILURE
}

#[cfg(unix)]
mod worker {
    use std::fs;
    use std::io;
    use std::process::ExitCode;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use anyhow::{Context, Result, anyhow};
    use epik::agent::{Budget, ClaudeCode, CodingAgent};
    use epik::chat::StopToken;
    use epik::config::{self, Config, Worker};
    use epik::git::Git;
    use epik::github::{GitHub, Repo};
    use epik::keystore::{GITHUB_OVERRIDE_ENV, OsKeyring};
    use epik::logging::{JsonLines, Log};
    use epik::preflight;
    use epik::run::{FeatureRun, FeatureVerdict, IssueRun, Retained, Verdict};
    use serde::Serialize;

    /// A conducted run whose verdict is failure — or one that could not be
    /// provisioned.
    const FAILED: u8 = 1;
    /// The command line is malformed.
    const USAGE: u8 = 2;
    /// The preflight refused: a capability is not present.
    const REFUSED: u8 = 3;
    /// Boot integrity: unparseable config, an unwritable Epik home.
    const BROKEN: u8 = 4;

    const USAGE_LINE: &str = "usage: epik-worker (--feature <n> | --issue <n>) --target <branch>";

    /// What each run may spend: wide now, tapering by evidence — no token
    /// or dollar ceiling yet. Claude Code's stream narrates every tool
    /// call, so ten minutes of true silence is a wedged run, not a slow
    /// one.
    const BUDGET: Budget = Budget {
        max_tokens: None,
        max_cost: None,
        stall: Duration::from_mins(10),
    };

    /// How long judgment waits for a check still running before calling the
    /// run failed: CI takes minutes, so half an hour is patience, not hope.
    const PATIENCE: Duration = Duration::from_mins(30);

    /// Which run one invocation conducts.
    enum Job {
        Feature,
        Issue,
    }

    struct Cli {
        job: Job,
        number: u64,
        target: String,
    }

    impl Cli {
        /// Hand-rolled, like every host here: two flags do not earn a
        /// dependency.
        fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
            let mut job: Option<(Job, u64)> = None;
            let mut target: Option<String> = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--feature" | "--issue" => {
                        let number = args
                            .next()
                            .and_then(|number| number.parse().ok())
                            .ok_or_else(|| format!("{arg} wants an issue number\n{USAGE_LINE}"))?;
                        let kind = if arg == "--feature" {
                            Job::Feature
                        } else {
                            Job::Issue
                        };
                        if job.replace((kind, number)).is_some() {
                            return Err(format!("one job per invocation\n{USAGE_LINE}"));
                        }
                    }
                    "--target" => {
                        let branch = args
                            .next()
                            .ok_or_else(|| format!("--target wants a branch\n{USAGE_LINE}"))?;
                        if target.replace(branch).is_some() {
                            return Err(format!("one target per invocation\n{USAGE_LINE}"));
                        }
                    }
                    stray => return Err(format!("unrecognized argument {stray:?}\n{USAGE_LINE}")),
                }
            }
            let (job, number) = job.ok_or_else(|| {
                format!("say which run: --feature <n> or --issue <n>\n{USAGE_LINE}")
            })?;
            let target = target
                .ok_or_else(|| format!("say where it merges: --target <branch>\n{USAGE_LINE}"))?;
            Ok(Self {
                job,
                number,
                target,
            })
        }
    }

    // `unreachable_pub` and nursery's `redundant_pub_crate` tug opposite
    // ways on a binary's one cross-module item; the narrower spelling wins.
    #[allow(clippy::redundant_pub_crate)]
    pub(super) fn main() -> ExitCode {
        let cli = match Cli::parse(std::env::args().skip(1)) {
            Ok(cli) => cli,
            Err(complaint) => {
                eprintln!("{complaint}");
                return ExitCode::from(USAGE);
            }
        };
        let (worker, repo, git) = match booted() {
            Ok(booted) => booted,
            Err(error) => {
                eprintln!("epik-worker: {error:#}");
                return ExitCode::from(BROKEN);
            }
        };
        let agent = ClaudeCode::at(&worker.agent);
        let token = match preflight::manifest(
            &agent.binaries(),
            std::env::var(GITHUB_OVERRIDE_ENV).ok(),
            &OsKeyring,
        ) {
            Ok(token) => token,
            Err(refusals) => {
                for refusal in &refusals {
                    eprintln!("refused: {refusal}");
                }
                return ExitCode::from(REFUSED);
            }
        };
        conducted(&cli, &repo, &git, &agent, token)
    }

    /// Boot integrity: the home made writable, the config read whole. The
    /// only failures fatal at startup — capability absences refuse and
    /// report instead.
    fn booted() -> Result<(Worker, Repo, Git)> {
        let home = config::home()?;
        fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
        let config = Config::load()?;
        let repo = Repo::parse(&config.worker.repo).ok_or_else(|| {
            anyhow!(
                "worker.repo {:?} is not an owner/name repository",
                config.worker.repo
            )
        })?;
        let git = Git::new()?;
        Ok((config.worker, repo, git))
    }

    /// Provisions the requested run and conducts it to its verdict.
    fn conducted(cli: &Cli, repo: &Repo, git: &Git, agent: &ClaudeCode, token: String) -> ExitCode {
        let github = GitHub::new(Some(token.clone()));
        // Credentials injected, never discovered: the wide prompt's agent
        // conducts the pull-request ceremony through gh, which answers to
        // either spelling.
        let env = vec![
            ("GH_TOKEN".to_owned(), token.clone()),
            ("GITHUB_TOKEN".to_owned(), token),
        ];
        let issue = match github.issue(repo, cli.number) {
            Ok(issue) => issue,
            Err(error) => {
                eprintln!("epik-worker: reading issue #{}: {error}", cli.number);
                return ExitCode::from(FAILED);
            }
        };
        // GitHub is the only rendezvous, so the clone URL is the repo's own
        // address — never a spelling with a token in it.
        let url = format!("https://github.com/{repo}.git");
        let stop = StopToken::new();
        match cli.job {
            Job::Feature => {
                let run = FeatureRun {
                    repo: repo.clone(),
                    url,
                    base: cli.target.clone(),
                    issue,
                    env,
                    budget: BUDGET,
                    patience: PATIENCE,
                };
                let Some(verdict) = pumped(|log| run.conduct(git, &github, agent, log, &stop))
                else {
                    return crashed();
                };
                match verdict {
                    FeatureVerdict::Done { review } => {
                        eprintln!("review at pull request #{review}");
                        ExitCode::SUCCESS
                    }
                    FeatureVerdict::Failed { report } => failed(&report),
                }
            }
            Job::Issue => {
                let run = IssueRun {
                    repo: repo.clone(),
                    url,
                    base: cli.target.clone(),
                    issue,
                    env,
                    budget: BUDGET,
                    patience: PATIENCE,
                };
                let Some(concluded) = pumped(|log| run.conduct(git, &github, agent, log, &stop))
                else {
                    return crashed();
                };
                match &concluded.retained {
                    Some(Retained::Forensics(worktree)) => {
                        eprintln!("worktree retained at {}", worktree.path().display());
                    }
                    Some(Retained::Undeleted(worktree, error)) => {
                        eprintln!(
                            "worktree at {} could not be removed: {error}",
                            worktree.path().display()
                        );
                    }
                    None => {}
                }
                match concluded.verdict {
                    Verdict::Done => ExitCode::SUCCESS,
                    Verdict::Failed { report } => failed(&report),
                }
            }
        }
    }

    /// Conducts `work` on its own thread — one thread per running agent,
    /// the host's own pattern — while this one pumps every event it emits
    /// to stdout, one JSON line each. `None` when the run panicked, which
    /// no verdict can stand for.
    fn pumped<E, V>(work: impl FnOnce(&mut dyn Log<E>) -> V + Send) -> Option<V>
    where
        E: Serialize + Send,
        V: Send,
    {
        thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel();
            let conducting = scope.spawn(move || {
                let mut log = sender;
                work(&mut log)
            });
            let mut lines = JsonLines::new(io::stdout().lock());
            for event in receiver {
                lines.emit(event);
            }
            conducting.join().ok()
        })
    }

    fn failed(report: &str) -> ExitCode {
        eprintln!("epik-worker: {report}");
        ExitCode::from(FAILED)
    }

    fn crashed() -> ExitCode {
        eprintln!("epik-worker: the run panicked");
        ExitCode::from(FAILED)
    }
}
