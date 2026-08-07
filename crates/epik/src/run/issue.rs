//! The old `epik:issue` loop, as a coded state machine.
//!
//! One [`IssueRun`] walks the [`Phase`]s in order and narrates each entry
//! into its [`RunEvent`] log. v1 runs wide: the whole middle of the loop is
//! one `TaskKind::Implement` invocation, the prompt the old skill text with
//! the worktree steps carved out — the harness provisions `Task.worktree` —
//! and the autonomy paragraph carved in. Done is never self-reported: after
//! the agent stops, the harness judges through the [`Evidence`] seam — pull
//! request merged into the target branch, checks green, issue closed — and
//! a [`Stop::Completed`] that GitHub disbelieves is a failed run with a
//! report. The worktree lifecycle is [`Worktree::conclude`]: removed on a
//! verified success, retained for forensics otherwise.

use crate::agent::{Budget, CodingAgent, Stop, Task, TaskKind};
use crate::chat::StopToken;
use crate::git::{Git, Outcome, Worktree};
use crate::github::{self, Check, Conclusion, GitHub, Issue, Pull, Repo, State};
use crate::logging::Log;
use crate::run::{Phase, RunEvent, Verdict};

/// The facts the verdict is judged from.
///
/// [`GitHub`] in production, a double in tests, which is what keeps the
/// state machine testable with no network. The seam supplies facts and
/// never conclusions — judgment stays in the run, so no double can fake it.
pub trait Evidence {
    /// The pull request `head` most recently produced, in whatever state.
    ///
    /// # Errors
    ///
    /// Returns a [`github::Error`] when GitHub cannot be asked or answers
    /// no.
    fn pull(&self, repo: &Repo, head: &str) -> Result<Option<Pull>, github::Error>;

    /// Every check run's verdict on `git_ref`.
    ///
    /// # Errors
    ///
    /// Returns a [`github::Error`] when GitHub cannot be asked or answers
    /// no.
    fn checks(&self, repo: &Repo, git_ref: &str) -> Result<Vec<Check>, github::Error>;

    /// The issue, read back.
    ///
    /// # Errors
    ///
    /// Returns a [`github::Error`] when GitHub cannot be asked or answers
    /// no.
    fn issue(&self, repo: &Repo, number: u64) -> Result<Issue, github::Error>;
}

impl Evidence for GitHub {
    fn pull(&self, repo: &Repo, head: &str) -> Result<Option<Pull>, github::Error> {
        self.pull_for(repo, head)
    }

    fn checks(&self, repo: &Repo, git_ref: &str) -> Result<Vec<Check>, github::Error> {
        self.check_conclusions(repo, git_ref)
    }

    fn issue(&self, repo: &Repo, number: u64) -> Result<Issue, github::Error> {
        // The inherent verb: inherent methods win the name over this trait's.
        Self::issue(self, repo, number)
    }
}

/// One issue, run to a verdict: the fully provisioned input to
/// [`conduct`](Self::conduct), on the [`Task`] precedent — everything
/// handed in, nothing discovered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueRun {
    pub repo: Repo,
    /// Where the repository is cloned from.
    pub url: String,
    /// The branch the work merges into.
    pub base: String,
    /// The issue as the caller read it: the prompt renders from this, and
    /// the verdict re-reads it through the seam.
    pub issue: Issue,
    /// Credentials injected, never discovered — [`Task::env`], passed
    /// through.
    pub env: Vec<(String, String)>,
    pub budget: Budget,
}

/// What the caller holds when a run is over: the verdict to render, and
/// the worktree when the retention policy kept it.
#[derive(Debug, Eq, PartialEq)]
pub struct Concluded {
    pub verdict: Verdict,
    /// `Some` on failure — forensics until dismissed — and on the one
    /// success path where deletion itself failed; `None` when a verified
    /// success cleaned up, or when no worktree ever existed.
    pub retained: Option<Worktree>,
}

impl IssueRun {
    /// The branch this run works on: deterministic, so a rerun reuses the
    /// name and [`Git::checkout`] resets whatever a wipe stranded.
    #[must_use]
    pub fn branch(&self) -> String {
        format!("issue-{}", self.issue.number)
    }

    /// Runs the whole loop to a verdict, narrating every state into `log`.
    ///
    /// Infallible by design: everything that goes wrong — git refusing,
    /// the agent stopping short, GitHub disbelieving a completion — is a
    /// [`Verdict::Failed`] with a report, never an error the caller must
    /// route separately.
    pub fn conduct(
        &self,
        git: &Git,
        evidence: &dyn Evidence,
        agent: &dyn CodingAgent,
        log: &mut dyn Log<RunEvent>,
        stop: &StopToken,
    ) -> Concluded {
        log.emit(RunEvent::Entered(Phase::Worktree));
        let worktree = match self.checked_out(git) {
            Ok(worktree) => worktree,
            // No worktree ever existed, so there is nothing to clean up or
            // retain.
            Err(error) => {
                let verdict = Verdict::Failed {
                    report: error.to_string(),
                };
                log.emit(RunEvent::Finished(verdict.clone()));
                return Concluded {
                    verdict,
                    retained: None,
                };
            }
        };
        let (verdict, outcome) = self.staged(&worktree, evidence, agent, log, stop);
        log.emit(RunEvent::Entered(Phase::Cleanup));
        // A deletion that fails hands the worktree back; keeping it is the
        // conservative end of the retention policy, not a new verdict.
        let retained = worktree
            .conclude(outcome)
            .unwrap_or_else(|(worktree, _)| Some(worktree));
        log.emit(RunEvent::Finished(verdict.clone()));
        Concluded { verdict, retained }
    }

    fn checked_out(&self, git: &Git) -> Result<Worktree, crate::git::Error> {
        git.fetch(&self.repo, &self.url)?;
        git.checkout(&self.repo, self.issue.number, &self.branch(), &self.base)
    }

    /// Implement through close: the wide agent invocation, then the
    /// verification states. What comes back is the verdict beside the
    /// [`Outcome`] the retention policy turns on.
    fn staged(
        &self,
        worktree: &Worktree,
        evidence: &dyn Evidence,
        agent: &dyn CodingAgent,
        log: &mut dyn Log<RunEvent>,
        stop: &StopToken,
    ) -> (Verdict, Outcome) {
        log.emit(RunEvent::Entered(Phase::Implement));
        let task = Task {
            kind: TaskKind::Implement,
            prompt: self.prompt(),
            worktree: worktree.path().to_owned(),
            env: self.env.clone(),
            budget: self.budget,
        };
        let run = agent.run(&task, &mut |event| log.emit(RunEvent::Agent(event)), stop);
        match run {
            Ok(Stop::Completed) => self.verified(worktree.branch(), evidence, log),
            Ok(Stop::Blocked { report }) => (Verdict::Failed { report }, Outcome::Blocked),
            Ok(Stop::Spent) => failed("the run spent its budget"),
            Ok(Stop::Stalled) => failed("the agent went silent past its stall window"),
            Ok(Stop::Canceled) => failed("the run was canceled"),
            Ok(Stop::Died { error }) => failed(format!("the agent died: {error}")),
            Err(error) => failed(error.to_string()),
        }
    }

    /// The verification states: a `Completed` stop is a belief, so the
    /// verdict is earned fact by fact through the [`Evidence`] seam.
    fn verified(
        &self,
        branch: &str,
        evidence: &dyn Evidence,
        log: &mut dyn Log<RunEvent>,
    ) -> (Verdict, Outcome) {
        match self.judged(branch, evidence, log) {
            Ok(()) => (Verdict::Done, Outcome::Succeeded),
            Err(report) => (Verdict::Failed { report }, Outcome::Failed),
        }
    }

    /// Watch through close, each state verifying the facet of done the
    /// wide prompt asked the agent to reach: checks green, merged into the
    /// target, issue closed. Review has nothing to verify in v1 — the
    /// agent reviewed inside implement — but the state is entered, because
    /// it is where the review taper will land.
    fn judged(
        &self,
        branch: &str,
        evidence: &dyn Evidence,
        log: &mut dyn Log<RunEvent>,
    ) -> Result<(), String> {
        log.emit(RunEvent::Entered(Phase::Watch));
        let pull = evidence
            .pull(&self.repo, branch)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no pull request has head {branch}"))?;
        let checks = evidence
            .checks(&self.repo, &pull.head.sha)
            .map_err(|error| error.to_string())?;
        green(&checks)?;
        log.emit(RunEvent::Entered(Phase::Review));
        log.emit(RunEvent::Entered(Phase::Merge));
        if !pull.merged {
            return Err(format!("pull request #{} is not merged", pull.number));
        }
        if pull.base.name != self.base {
            return Err(format!(
                "pull request #{} merged into {}, not {}",
                pull.number, pull.base.name, self.base
            ));
        }
        log.emit(RunEvent::Entered(Phase::Close));
        let issue = evidence
            .issue(&self.repo, self.issue.number)
            .map_err(|error| error.to_string())?;
        match issue.state {
            State::Closed => Ok(()),
            State::Open => Err(format!("issue #{} is still open", self.issue.number)),
        }
    }

    /// The wide v1 prompt: the old `epik:issue` skill text, lightly
    /// edited. The worktree steps are out — provisioning and cleanup are
    /// the harness's own states — and the autonomy paragraph is in, the
    /// belt to the vocabulary's suspenders: there is no `Ask` variant to
    /// stop and wait on.
    fn prompt(&self) -> String {
        format!(
            "You are implementing GitHub issue #{number} of {repo}: {title}

{body}

Your working directory is a git worktree prepared for you, checked out on
branch `{branch}`. The work merges into `{base}`.

1. Implement the issue. Get it working locally and make sure all tests
   pass.
2. Create a pull request targeting `{base}`. Monitor the pull request for
   problems and fix them as they occur. Problems may include: errors in
   the continuous integration pipeline; source conflicts; a base branch
   that needs to be refreshed.
3. When the pull request is ready to merge, run a code review, writing
   review comments to the GitHub issue, addressing all comments, and
   fixing further continuous integration problems as necessary.
4. Merge the pull request branch into the target branch.
5. Make sure that the issue is closed.

Work autonomously: never stop to ask a question, because nobody will
answer. When you are truly blocked, fail loudly: stop and report exactly
what blocked you.",
            number = self.issue.number,
            repo = self.repo,
            title = self.issue.title,
            body = self.issue.body,
            branch = self.branch(),
            base = self.base,
        )
    }
}

fn failed(report: impl Into<String>) -> (Verdict, Outcome) {
    (
        Verdict::Failed {
            report: report.into(),
        },
        Outcome::Failed,
    )
}

/// Green, or the reason not: a check still running or concluded anything
/// but success keeps the commit red. Neutral and skipped count as green,
/// matching how GitHub's own merge gate reads them.
fn green(checks: &[Check]) -> Result<(), String> {
    let red: Vec<&str> = checks
        .iter()
        .filter(|check| {
            !matches!(
                check.conclusion,
                Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)
            )
        })
        .map(|check| check.name.as_str())
        .collect();
    if red.is_empty() {
        Ok(())
    } else {
        Err(format!("checks not green: {}", red.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::agent::{AgentEvent, Play, Scripted};
    use crate::event::Usage;
    use crate::github::Branch;

    /// Runs git in `dir`, insisting it succeed: origin-side scaffolding,
    /// hermetic like the git module's own spawns.
    fn sh(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "epik")
            .env("GIT_AUTHOR_EMAIL", "epik@example.invalid")
            .env("GIT_COMMITTER_NAME", "epik")
            .env("GIT_COMMITTER_EMAIL", "epik@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A little GitHub for git alone: a local origin with one commit, and
    /// a cache rooted beside it. What GitHub the API would say comes from
    /// [`Canned`] instead — no network anywhere.
    struct World {
        root: TempDir,
        git: Git,
        repo: Repo,
        url: String,
    }

    fn world() -> World {
        let root = TempDir::new().unwrap();
        let origin = root.path().join("github");
        fs::create_dir_all(&origin).unwrap();
        sh(&origin, &["init", "-b", "main"]);
        fs::write(origin.join("README.md"), "hello").unwrap();
        sh(&origin, &["add", "."]);
        sh(&origin, &["commit", "-m", "hello"]);
        let git = Git::rooted(root.path().join("repos"), root.path().join("work"));
        let url = origin.display().to_string();
        World {
            root,
            git,
            repo: Repo::new("epik-agent", "Epik"),
            url,
        }
    }

    fn run(world: &World, number: u64) -> IssueRun {
        IssueRun {
            repo: world.repo.clone(),
            url: world.url.clone(),
            base: "main".to_owned(),
            issue: Issue {
                number,
                title: "make it work".to_owned(),
                body: "the details".to_owned(),
                state: State::Open,
            },
            env: Vec::new(),
            budget: Budget {
                max_tokens: None,
                max_cost: None,
                stall: Duration::from_mins(1),
            },
        }
    }

    /// The facts, canned: whatever the test says GitHub would say.
    struct Canned {
        pull: Option<Pull>,
        checks: Vec<Check>,
        issue: Issue,
    }

    impl Evidence for Canned {
        fn pull(&self, _: &Repo, _: &str) -> Result<Option<Pull>, github::Error> {
            Ok(self.pull.clone())
        }

        fn checks(&self, _: &Repo, _: &str) -> Result<Vec<Check>, github::Error> {
            Ok(self.checks.clone())
        }

        fn issue(&self, _: &Repo, _: u64) -> Result<Issue, github::Error> {
            Ok(self.issue.clone())
        }
    }

    /// Evidence that everything the wide prompt asked for actually
    /// happened: merged into the base, checks green, issue closed.
    fn convinced(run: &IssueRun) -> Canned {
        Canned {
            pull: Some(Pull {
                number: 41,
                title: "the work".to_owned(),
                state: State::Closed,
                merged: true,
                head: Branch {
                    name: run.branch(),
                    sha: "a".repeat(40),
                },
                base: Branch {
                    name: run.base.clone(),
                    sha: "b".repeat(40),
                },
            }),
            checks: vec![
                Check {
                    name: "CI".to_owned(),
                    conclusion: Some(Conclusion::Success),
                },
                Check {
                    name: "deploy-website".to_owned(),
                    conclusion: Some(Conclusion::Skipped),
                },
            ],
            issue: Issue {
                state: State::Closed,
                ..run.issue.clone()
            },
        }
    }

    fn completing() -> Scripted {
        Scripted::playing(vec![
            Play::Progress("implementing".to_owned()),
            Play::Finish(Stop::Completed),
        ])
    }

    fn conducted(
        run: &IssueRun,
        world: &World,
        evidence: &dyn Evidence,
        agent: &dyn CodingAgent,
    ) -> (Vec<RunEvent>, Concluded) {
        let mut events: Vec<RunEvent> = Vec::new();
        let concluded = run.conduct(&world.git, evidence, agent, &mut events, &StopToken::new());
        assert_eq!(
            events.last(),
            Some(&RunEvent::Finished(concluded.verdict.clone())),
            "the log's last word and the returned verdict tell one story"
        );
        (events, concluded)
    }

    fn phases(events: &[RunEvent]) -> Vec<Phase> {
        events
            .iter()
            .filter_map(|event| match event {
                RunEvent::Entered(phase) => Some(*phase),
                _ => None,
            })
            .collect()
    }

    fn report(concluded: &Concluded) -> &str {
        match &concluded.verdict {
            Verdict::Failed { report } => report,
            Verdict::Done => panic!("expected a failed run: {concluded:?}"),
        }
    }

    #[test]
    fn a_verified_run_narrates_every_state_and_removes_its_worktree() {
        let w = world();
        let r = run(&w, 7);
        let agent = Scripted::playing(vec![
            Play::Progress("implementing".to_owned()),
            Play::Usage(Usage::tokens(18, 6)),
            Play::Finish(Stop::Completed),
        ]);

        let (events, concluded) = conducted(&r, &w, &convinced(&r), &agent);

        assert_eq!(concluded.verdict, Verdict::Done);
        assert_eq!(concluded.retained, None);
        assert!(
            !w.git.worktree(&w.repo, 7).exists(),
            "a verified success leaves no worktree"
        );
        assert_eq!(
            phases(&events),
            [
                Phase::Worktree,
                Phase::Implement,
                Phase::Watch,
                Phase::Review,
                Phase::Merge,
                Phase::Close,
                Phase::Cleanup,
            ],
            "every state transition is observable, in loop order"
        );
        let carried: Vec<&AgentEvent> = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::Agent(inner) => Some(inner),
                _ => None,
            })
            .collect();
        assert!(
            matches!(carried.first(), Some(AgentEvent::Started { .. })),
            "the agent's whole stream rides within the run's: {carried:?}"
        );
        assert!(carried.contains(&&AgentEvent::Progress("implementing".to_owned())));
        assert_eq!(
            carried.last(),
            Some(&&AgentEvent::Finished(Stop::Completed))
        );
    }

    #[test]
    fn a_blocked_agent_fails_the_run_with_its_own_report_and_keeps_the_worktree() {
        let w = world();
        let r = run(&w, 8);
        let agent = Scripted::playing(vec![Play::Finish(Stop::Blocked {
            report: "the issue asks for two contradictory things".to_owned(),
        })]);

        let (events, concluded) = conducted(&r, &w, &convinced(&r), &agent);

        assert_eq!(
            concluded.verdict,
            Verdict::Failed {
                report: "the issue asks for two contradictory things".to_owned()
            }
        );
        assert!(
            concluded.retained.is_some() && w.git.worktree(&w.repo, 8).is_dir(),
            "a blocked run's worktree is forensic evidence"
        );
        assert_eq!(
            phases(&events),
            [Phase::Worktree, Phase::Implement, Phase::Cleanup],
            "nothing is verified for an agent that never claimed completion"
        );
    }

    #[test]
    fn a_completed_stop_that_github_disbelieves_is_a_failed_run() {
        let w = world();
        let r = run(&w, 9);
        let mut evidence = convinced(&r);
        evidence.pull.as_mut().unwrap().merged = false;

        let (events, concluded) = conducted(&r, &w, &evidence, &completing());

        assert!(report(&concluded).contains("not merged"), "{concluded:?}");
        assert!(
            concluded.retained.is_some() && w.git.worktree(&w.repo, 9).is_dir(),
            "self-reported success earns no cleanup"
        );
        assert_eq!(
            phases(&events),
            [
                Phase::Worktree,
                Phase::Implement,
                Phase::Watch,
                Phase::Review,
                Phase::Merge,
                Phase::Cleanup,
            ],
            "the log shows exactly which verification state disbelieved"
        );
    }

    #[test]
    fn each_verification_shortfall_names_its_fact() {
        type Tweak = fn(&mut Canned);
        let w = world();
        let cases: [(Tweak, &str); 4] = [
            (|canned| canned.pull = None, "no pull request"),
            (
                |canned| {
                    canned.checks = vec![Check {
                        name: "CI".to_owned(),
                        conclusion: Some(Conclusion::Failure),
                    }];
                },
                "checks not green: CI",
            ),
            (
                |canned| {
                    canned.checks = vec![Check {
                        name: "CI".to_owned(),
                        conclusion: None,
                    }];
                },
                "checks not green: CI",
            ),
            (|canned| canned.issue.state = State::Open, "still open"),
        ];
        for (number, (tweak, needle)) in (20..).zip(cases) {
            let r = run(&w, number);
            let mut evidence = convinced(&r);
            tweak(&mut evidence);

            let (_, concluded) = conducted(&r, &w, &evidence, &completing());

            assert!(
                report(&concluded).contains(needle),
                "wanted {needle:?} in {concluded:?}"
            );
            assert!(concluded.retained.is_some());
        }
    }

    #[test]
    fn a_pull_merged_somewhere_other_than_the_target_does_not_count() {
        let w = world();
        let r = run(&w, 30);
        let mut evidence = convinced(&r);
        evidence.pull.as_mut().unwrap().base.name = "trunk".to_owned();

        let (_, concluded) = conducted(&r, &w, &evidence, &completing());

        assert!(
            report(&concluded).contains("trunk, not main"),
            "{concluded:?}"
        );
    }

    #[test]
    fn a_spent_run_is_failed_and_kept_for_forensics() {
        let w = world();
        let mut r = run(&w, 10);
        r.budget.max_tokens = Some(10);
        let agent = Scripted::playing(vec![
            Play::Usage(Usage::tokens(90, 20)),
            Play::Finish(Stop::Completed),
        ]);

        let (_, concluded) = conducted(&r, &w, &convinced(&r), &agent);

        assert!(report(&concluded).contains("budget"), "{concluded:?}");
        assert!(concluded.retained.is_some() && w.git.worktree(&w.repo, 10).is_dir());
    }

    #[test]
    fn a_run_that_cannot_get_a_worktree_fails_before_implementing() {
        let w = world();
        let mut r = run(&w, 3);
        r.url = w.root.path().join("no-such-origin").display().to_string();

        let (events, concluded) = conducted(&r, &w, &convinced(&r), &completing());

        assert!(matches!(concluded.verdict, Verdict::Failed { .. }));
        assert_eq!(concluded.retained, None, "no worktree ever existed");
        assert_eq!(
            phases(&events),
            [Phase::Worktree],
            "the run never reached the agent"
        );
    }

    #[test]
    fn the_wide_prompt_carries_the_issue_and_the_autonomy_paragraph() {
        let w = world();
        let prompt = run(&w, 5).prompt();

        for expected in [
            "#5",
            "epik-agent/Epik",
            "make it work",
            "the details",
            "`issue-5`",
            "`main`",
            "fail loudly",
            "never stop to ask",
        ] {
            assert!(prompt.contains(expected), "{expected:?} not in:\n{prompt}");
        }
        for carved_out in ["Create a git worktree", "Clean up"] {
            assert!(
                !prompt.contains(carved_out),
                "the worktree lifecycle is the harness's, not the prompt's:\n{prompt}"
            );
        }
    }
}
