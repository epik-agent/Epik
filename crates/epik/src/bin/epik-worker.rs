//! The worker: Epik's runs, as a local process.
//!
//! A thin main over the library, as the coding-agents ADR promises: read
//! config, resolve keys, preflight, conduct, pump. One run per invocation —
//! `--feature <n>` drains a feature's sub-issue graph, `--issue <n>`
//! conducts one issue — against `--target <branch>`, the agent on its own
//! thread while this one pumps every event to stdout as JSON lines:
//! `FeatureEvent`s for a feature, `RunEvent`s for an issue, through the
//! same `Log<E>` machinery every host pumps. The same lines land in the
//! run's log beneath `~/.epik/logs` ([`epik::logs`]) — stdout is for
//! whoever is listening now, the file for whoever asks later.
//!
//! Before any run, the preflight resolves the config-derived manifest
//! ([`epik::preflight`]) and refuses with rendered `CapabilityStatus`es —
//! nothing on stdout, no backtrace, no worktree, no log file. SIGINT and
//! SIGTERM are wired to the run's stop token, so cancellation winds the
//! run up through the governed agent's own group kill — never an orphaned,
//! credentialed claude outliving its harness. Boot-integrity failures
//! (unparseable config, an unwritable Epik home, a run log that cannot be
//! opened) are the only fatal-before-the-run class, and the exit code
//! tells every class apart:
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
    use std::fs::File;
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
    use epik::logging::{Both, JsonLines, Log};
    use epik::logs::{Kind, Logs};
    use epik::preflight;
    use epik::run::{
        FeatureEvent, FeatureRun, FeatureVerdict, IssueRun, Retained, RunEvent, Verdict,
    };
    use serde::Serialize;

    /// A conducted run whose verdict is failure — or one that could not be
    /// provisioned.
    const FAILED: u8 = 1;
    /// The command line is malformed.
    const USAGE: u8 = 2;
    /// The preflight refused: a capability is not present.
    const REFUSED: u8 = 3;
    /// Boot integrity: unparseable config, an unwritable Epik home, a run
    /// log that cannot be opened.
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

    struct Cli {
        /// Which run one invocation conducts — [`Kind`] doubles as the
        /// worker's job vocabulary, being exactly the same two-way choice
        /// the log file is named by.
        job: Kind,
        number: u64,
        target: String,
    }

    impl Cli {
        /// Hand-rolled, like every host here: two flags do not earn a
        /// dependency.
        fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
            let mut job: Option<(Kind, u64)> = None;
            let mut target: Option<String> = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--feature" | "--issue" => {
                        let number = args
                            .next()
                            .and_then(|number| number.parse().ok())
                            .ok_or_else(|| format!("{arg} wants an issue number\n{USAGE_LINE}"))?;
                        let kind = if arg == "--feature" {
                            Kind::Feature
                        } else {
                            Kind::Issue
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
        let stop = StopToken::new();
        if let Err(error) = canceling(&stop) {
            eprintln!("epik-worker: wiring cancellation: {error}");
            return ExitCode::from(BROKEN);
        }
        // The vouched-for token, offered onward: to git for the fetches,
        // through the askpass rails.
        let git = git.authenticated(token.clone());
        conducted(&cli, &worker, &repo, &git, &agent, token, &stop)
    }

    /// Wires SIGINT and SIGTERM to the run's stop token. A signal asks the
    /// run to wind up: the governed agent answers within its glance, kills
    /// its whole process group, and the worker exits by the verdict — a
    /// cancelled worker never leaves an orphaned, credentialed claude
    /// running past its harness.
    fn canceling(stop: &StopToken) -> std::io::Result<()> {
        use signal_hook::consts::{SIGINT, SIGTERM};
        let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM])?;
        let stop = stop.clone();
        thread::spawn(move || {
            for _ in signals.forever() {
                stop.stop();
            }
        });
        Ok(())
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
    fn conducted(
        cli: &Cli,
        worker: &Worker,
        repo: &Repo,
        git: &Git,
        agent: &ClaudeCode,
        token: String,
        stop: &StopToken,
    ) -> ExitCode {
        let github = worker.api.as_ref().map_or_else(
            || GitHub::new(Some(token.clone())),
            |api| GitHub::at(api, Some(token.clone())),
        );
        // Credentials injected, never discovered: the wide prompt's agent
        // conducts the pull-request ceremony through gh, which answers to
        // either spelling — and commits as Epik, so a box with no
        // ~/.gitconfig never burns agent time on "tell me who you are".
        let env = vec![
            ("GH_TOKEN".to_owned(), token.clone()),
            ("GITHUB_TOKEN".to_owned(), token),
            ("GIT_AUTHOR_NAME".to_owned(), "Epik".to_owned()),
            ("GIT_AUTHOR_EMAIL".to_owned(), "epik@localhost".to_owned()),
            ("GIT_COMMITTER_NAME".to_owned(), "Epik".to_owned()),
            (
                "GIT_COMMITTER_EMAIL".to_owned(),
                "epik@localhost".to_owned(),
            ),
        ];
        // GitHub is the only rendezvous, so the clone URL defaults to the
        // repo's own address — never a spelling with a token in it; the
        // token rides the cache's askpass rails instead.
        let url = worker
            .url
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{repo}.git"));
        match cli.job {
            Kind::Feature => {
                // No prefetch: a feature run reads its own graph, one
                // snapshot serving the whole run.
                let run = FeatureRun {
                    repo: repo.clone(),
                    url,
                    base: cli.target.clone(),
                    number: cli.number,
                    env,
                    budget: BUDGET,
                    patience: PATIENCE,
                };
                let log = match opened(repo, Kind::Feature, cli.number) {
                    Ok(log) => log,
                    Err(code) => return code,
                };
                let verdict = pumped(
                    log,
                    |log| run.conduct(git, &github, agent, log, stop),
                    || {
                        FeatureEvent::Finished(FeatureVerdict::Failed {
                            report: CRASHED.to_owned(),
                        })
                    },
                );
                let Some(verdict) = verdict else {
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
            Kind::Issue => {
                // The one REST prefetch: an issue run is fully provisioned,
                // everything handed in, so the issue is read here.
                let issue = match github.issue(repo, cli.number) {
                    Ok(issue) => issue,
                    Err(error) => {
                        eprintln!("epik-worker: reading issue #{}: {error}", cli.number);
                        return ExitCode::from(FAILED);
                    }
                };
                let run = IssueRun {
                    repo: repo.clone(),
                    url,
                    base: cli.target.clone(),
                    issue,
                    env,
                    budget: BUDGET,
                    patience: PATIENCE,
                };
                let log = match opened(repo, Kind::Issue, cli.number) {
                    Ok(log) => log,
                    Err(code) => return code,
                };
                let concluded = pumped(
                    log,
                    |log| run.conduct(git, &github, agent, log, stop),
                    || {
                        RunEvent::Finished(Verdict::Failed {
                            report: CRASHED.to_owned(),
                        })
                    },
                );
                let Some(concluded) = concluded else {
                    return crashed();
                };
                reported(concluded.retained.as_ref());
                match concluded.verdict {
                    Verdict::Done => ExitCode::SUCCESS,
                    Verdict::Failed { report } => failed(&report),
                }
            }
        }
    }

    /// Says on stderr where a kept worktree stands, and why.
    fn reported(retained: Option<&Retained>) {
        match retained {
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
    }

    /// Opens the run's log, deliberately fail-closed: the file is the only
    /// durable copy of the narration, so a run whose log cannot exist must
    /// not start — an unopenable log is boot integrity, exit code
    /// [`BROKEN`]. Called at the last moment before the first event, after
    /// everything that can refuse — the preflight, an issue run's prefetch
    /// — because a run that never started leaves no file saying otherwise.
    fn opened(repo: &Repo, kind: Kind, number: u64) -> Result<JsonLines<File>, ExitCode> {
        Logs::new()
            .and_then(|logs| logs.create(repo, kind, number))
            .map_err(|error| {
                eprintln!("epik-worker: {error:#}");
                ExitCode::from(BROKEN)
            })
    }

    /// Conducts `work` on its own thread — one thread per running agent,
    /// the host's own pattern — while this one pumps every event it emits
    /// to two sinks, one JSON line each: stdout for whoever is listening
    /// now, `file` for whoever asks later. `None` when the run panicked,
    /// which no verdict can stand for — though the sinks still get `crash`
    /// as their last word, because a narration that just stops mid-stream
    /// reads as an unfinished run rather than a dead one.
    fn pumped<E, V>(
        file: JsonLines<File>,
        work: impl FnOnce(&mut dyn Log<E>) -> V + Send,
        crash: impl FnOnce() -> E,
    ) -> Option<V>
    where
        E: Clone + Serialize + Send,
        V: Send,
    {
        thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel();
            let conducting = scope.spawn(move || {
                let mut log = sender;
                work(&mut log)
            });
            let mut sinks = Both(JsonLines::new(io::stdout().lock()), file);
            for event in receiver {
                sinks.emit(event);
            }
            let verdict = conducting.join().ok();
            if verdict.is_none() {
                sinks.emit(crash());
            }
            // The file is the only durable copy of the narration: the
            // verdict is GitHub's truth and stands whatever became of the
            // log, but a lost line is said aloud, never swallowed.
            if let Some(error) = sinks.1.failure() {
                eprintln!("epik-worker: the run log lost events: {error}");
            }
            verdict
        })
    }

    fn failed(report: &str) -> ExitCode {
        eprintln!("epik-worker: {report}");
        ExitCode::from(FAILED)
    }

    /// What a panicked run reports, in the log's last word and on stderr.
    const CRASHED: &str = "the run panicked";

    fn crashed() -> ExitCode {
        eprintln!("epik-worker: {CRASHED}");
        ExitCode::from(FAILED)
    }
}
