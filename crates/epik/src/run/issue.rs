//! The old `epik:issue` loop, as a coded state machine.
//!
//! One [`IssueRun`] walks the [`Phase`]s in order and narrates each entry
//! into its [`RunEvent`] log. v1 runs wide: the whole middle of the loop is
//! one `TaskKind::Implement` invocation, the prompt the old skill text with
//! the worktree steps carved out — the harness provisions `Task.worktree` —
//! and the autonomy paragraph carved in. Done is never self-reported: after
//! the agent stops, the harness judges through the [`Evidence`] seam — the
//! pull request standing on the very commit this run's worktree produced,
//! merged into the target branch, checks green, issue closed — and a
//! [`Stop::Completed`] that GitHub disbelieves is a failed run with a
//! report. The worktree lifecycle is [`Worktree::conclude`]: removed on a
//! verified success, retained for forensics otherwise.

use std::thread;
use std::time::{Duration, Instant};

use crate::agent::{Budget, CodingAgent, Stop, Task, TaskKind};
use crate::chat::StopToken;
use crate::git::{self, Git, Outcome, Worktree};
use crate::github::{self, Check, GitHub, Issue, Pull, Repo, State};
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
    /// How long judgment waits for a check still running before the run is
    /// called failed: a merged pull request with a trailing workflow is
    /// not-yet-judgeable, never red. Bounded and synchronous, like
    /// everything.
    pub patience: Duration,
}

/// What the caller holds when a run is over: the verdict to render, and
/// the worktree when the retention policy kept it.
#[derive(Debug, Eq, PartialEq)]
pub struct Concluded {
    pub verdict: Verdict,
    /// `None` when a verified success cleaned up, or when no worktree ever
    /// existed.
    pub retained: Option<Retained>,
}

/// The worktree a finished run still holds, and why.
#[derive(Debug, Eq, PartialEq)]
pub enum Retained {
    /// Kept deliberately: forensics for a run that failed, until the next
    /// run of the same issue — or the user — dismisses it.
    Forensics(Worktree),
    /// Kept accidentally: a verified success whose deletion itself failed.
    /// The error beside the worktree is the explanation, and dismissal can
    /// be retried.
    Undeleted(Worktree, git::Error),
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
        let (verdict, retained) = match self.checked_out(git) {
            // No worktree ever existed: nothing to clean, nothing to retain.
            Err(error) => (
                Verdict::Failed {
                    report: error.to_string(),
                },
                None,
            ),
            Ok(worktree) => {
                let (verdict, outcome) = self.staged(&worktree, evidence, agent, log, stop);
                log.emit(RunEvent::Entered(Phase::Cleanup));
                let retained = match worktree.conclude(outcome) {
                    Ok(kept) => kept.map(Retained::Forensics),
                    // A deletion that fails hands the worktree back beside
                    // the error: kept, and explained, never silently leaked.
                    Err((worktree, error)) => Some(Retained::Undeleted(worktree, error)),
                };
                (verdict, retained)
            }
        };
        log.emit(RunEvent::Finished(verdict.clone()));
        Concluded { verdict, retained }
    }

    fn checked_out(&self, git: &Git) -> Result<Worktree, git::Error> {
        git.fetch(&self.repo, &self.url)?;
        // A rerun's predecessor has had its forensic chance: whatever
        // worktree a failed run left behind is dismissed here, so a retry
        // reaches the agent instead of refusing at checkout forever.
        if let Some(stale) = git.retained(&self.repo, self.issue.number, &self.branch()) {
            stale.dismiss().map_err(|(_, error)| error)?;
        }
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
            Ok(Stop::Completed) => match worktree.head() {
                Ok(head) => self.verified(worktree.branch(), &head, evidence, log),
                Err(error) => failed(error.to_string()),
            },
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
        head: &str,
        evidence: &dyn Evidence,
        log: &mut dyn Log<RunEvent>,
    ) -> (Verdict, Outcome) {
        match self.judged(branch, head, evidence, log) {
            Ok(()) => (Verdict::Done, Outcome::Succeeded),
            Err(report) => (Verdict::Failed { report }, Outcome::Failed),
        }
    }

    /// Watch through close, each state verifying the facet of done the
    /// wide prompt asked the agent to reach: the pull request standing on
    /// `head` — the commit the worktree produced, which is what binds the
    /// verdict to this run's work rather than a predecessor's merged pull
    /// request — checks green, merged into the target, issue closed.
    /// Review has nothing to verify in v1 — the agent reviewed inside
    /// implement — but the state is entered, because it is where the
    /// review taper will land.
    fn judged(
        &self,
        branch: &str,
        head: &str,
        evidence: &dyn Evidence,
        log: &mut dyn Log<RunEvent>,
    ) -> Result<(), String> {
        log.emit(RunEvent::Entered(Phase::Watch));
        let pull = evidence
            .pull(&self.repo, branch)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no pull request has head {branch}"))?;
        if pull.head.sha != head {
            return Err(format!(
                "pull request #{} stands at {}, not this run's work at {head}",
                pull.number, pull.head.sha
            ));
        }
        red(&self.settled(evidence, &pull.head.sha)?)?;
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

    /// The checks on `sha`, waited into a judgeable state: a pending
    /// conclusion is re-read rather than called red, until none are
    /// pending or the run's [`patience`](IssueRun::patience) is spent —
    /// at which point the still-running checks are the report, honestly
    /// distinct from red ones.
    fn settled(&self, evidence: &dyn Evidence, sha: &str) -> Result<Vec<Check>, String> {
        let deadline = Instant::now() + self.patience;
        loop {
            let checks = evidence
                .checks(&self.repo, sha)
                .map_err(|error| error.to_string())?;
            let pending: Vec<&str> = checks
                .iter()
                .filter(|check| check.pending())
                .map(|check| check.name.as_str())
                .collect();
            if pending.is_empty() {
                return Ok(checks);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!("checks still running: {}", pending.join(", ")));
            }
            thread::sleep((self.patience / 10).min(deadline - now));
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

/// The concluded-and-red checks, named — the pending ones were already
/// waited out by [`IssueRun::settled`], so red here really means red.
fn red(checks: &[Check]) -> Result<(), String> {
    let red: Vec<&str> = checks
        .iter()
        .filter(|check| check.red())
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
    use std::cell::RefCell;

    use super::*;
    use crate::agent::{AgentEvent, Play, Scripted};
    use crate::event::Usage;
    use crate::git::testing::{World, rev};
    use crate::github::{Branch, Conclusion};

    fn run(world: &World, number: u64) -> IssueRun {
        IssueRun {
            repo: world.repo.clone(),
            url: world.url(),
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
            patience: Duration::ZERO,
        }
    }

    /// What a run's worktree will stand on when the scripted agent commits
    /// nothing: the origin's own tip.
    fn main_sha(world: &World) -> String {
        rev(&world.origin, "main")
    }

    /// The facts, canned: whatever the test says GitHub would say.
    /// `checks` holds successive answers — drained from the front, the
    /// last one sticky — so a test can stage a check that concludes.
    struct Canned {
        pull: Option<Pull>,
        checks: RefCell<Vec<Vec<Check>>>,
        issue: Issue,
    }

    impl Evidence for Canned {
        fn pull(&self, _: &Repo, _: &str) -> Result<Option<Pull>, github::Error> {
            Ok(self.pull.clone())
        }

        fn checks(&self, _: &Repo, _: &str) -> Result<Vec<Check>, github::Error> {
            let mut answers = self.checks.borrow_mut();
            if answers.len() > 1 {
                Ok(answers.remove(0))
            } else {
                Ok(answers.first().cloned().unwrap_or_default())
            }
        }

        fn issue(&self, _: &Repo, _: u64) -> Result<Issue, github::Error> {
            Ok(self.issue.clone())
        }
    }

    fn green_checks() -> Vec<Check> {
        vec![
            Check {
                name: "CI".to_owned(),
                conclusion: Some(Conclusion::Success),
            },
            Check {
                name: "deploy-website".to_owned(),
                conclusion: Some(Conclusion::Skipped),
            },
        ]
    }

    /// Evidence that everything the wide prompt asked for actually
    /// happened *to this run's work*: a pull request standing at `head`,
    /// merged into the base, checks green, issue closed.
    fn convinced(run: &IssueRun, head: &str) -> Canned {
        Canned {
            pull: Some(Pull {
                number: 41,
                title: "the work".to_owned(),
                state: State::Closed,
                merged: true,
                head: Branch {
                    name: run.branch(),
                    sha: head.to_owned(),
                },
                base: Branch {
                    name: run.base.clone(),
                    sha: "b".repeat(40),
                },
            }),
            checks: RefCell::new(vec![green_checks()]),
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
        let w = World::new();
        let r = run(&w, 7);
        let agent = Scripted::playing(vec![
            Play::Progress("implementing".to_owned()),
            Play::Usage(Usage::tokens(18, 6)),
            Play::Finish(Stop::Completed),
        ]);

        let (events, concluded) = conducted(&r, &w, &convinced(&r, &main_sha(&w)), &agent);

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
        let w = World::new();
        let r = run(&w, 8);
        let agent = Scripted::playing(vec![Play::Finish(Stop::Blocked {
            report: "the issue asks for two contradictory things".to_owned(),
        })]);

        let (events, concluded) = conducted(&r, &w, &convinced(&r, &main_sha(&w)), &agent);

        assert_eq!(
            concluded.verdict,
            Verdict::Failed {
                report: "the issue asks for two contradictory things".to_owned()
            }
        );
        assert!(
            matches!(concluded.retained, Some(Retained::Forensics(_)))
                && w.git.worktree(&w.repo, 8).is_dir(),
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
        let w = World::new();
        let r = run(&w, 9);
        let mut evidence = convinced(&r, &main_sha(&w));
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
    fn a_predecessors_merged_pull_cannot_stand_in_for_this_runs_work() {
        let w = World::new();
        let r = run(&w, 11);
        // The reopened-issue scenario: a previous run's pull request is
        // merged and the issue reads closed, but this run's agent produced
        // nothing — the worktree stands on the base, not on the pull
        // request's head.
        let evidence = convinced(&r, &"c".repeat(40));

        let (_, concluded) = conducted(&r, &w, &evidence, &completing());

        assert!(
            report(&concluded).contains("not this run's work"),
            "{concluded:?}"
        );
        assert!(
            concluded.retained.is_some() && w.git.worktree(&w.repo, 11).is_dir(),
            "an unverified run earns no cleanup"
        );
    }

    #[test]
    fn a_rerun_dismisses_the_stale_forensics_and_reaches_the_agent() {
        let w = World::new();
        let r = run(&w, 12);
        let blocked = Scripted::playing(vec![Play::Finish(Stop::Blocked {
            report: "stuck".to_owned(),
        })]);
        let (_, first) = conducted(&r, &w, &convinced(&r, &main_sha(&w)), &blocked);
        assert!(first.retained.is_some(), "the failed run kept its worktree");

        let (events, second) = conducted(&r, &w, &convinced(&r, &main_sha(&w)), &completing());

        assert!(
            phases(&events).contains(&Phase::Implement),
            "the rerun reached the agent instead of refusing at checkout"
        );
        assert_eq!(
            second.verdict,
            Verdict::Done,
            "the retained forensics had their chance"
        );
        assert!(!w.git.worktree(&w.repo, 12).exists());
    }

    #[test]
    fn a_pending_check_is_waited_out_rather_than_judged_red() {
        let w = World::new();
        let mut r = run(&w, 13);
        r.patience = Duration::from_secs(5);
        let evidence = convinced(&r, &main_sha(&w));
        *evidence.checks.borrow_mut() = vec![
            vec![Check {
                name: "CI".to_owned(),
                conclusion: None,
            }],
            green_checks(),
        ];

        let (_, concluded) = conducted(&r, &w, &evidence, &completing());

        assert_eq!(
            concluded.verdict,
            Verdict::Done,
            "a trailing workflow is not-yet-judgeable, never red"
        );
    }

    #[test]
    fn each_verification_shortfall_names_its_fact() {
        type Tweak = fn(&mut Canned);
        let w = World::new();
        let cases: [(Tweak, &str); 4] = [
            (|canned| canned.pull = None, "no pull request"),
            (
                |canned| {
                    canned.checks = RefCell::new(vec![vec![Check {
                        name: "CI".to_owned(),
                        conclusion: Some(Conclusion::Failure),
                    }]]);
                },
                "checks not green: CI",
            ),
            (
                // With zero patience, a check that never concludes is the
                // report — named as still running, not as red.
                |canned| {
                    canned.checks = RefCell::new(vec![vec![Check {
                        name: "CI".to_owned(),
                        conclusion: None,
                    }]]);
                },
                "checks still running: CI",
            ),
            (|canned| canned.issue.state = State::Open, "still open"),
        ];
        for (number, (tweak, needle)) in (20..).zip(cases) {
            let r = run(&w, number);
            let mut evidence = convinced(&r, &main_sha(&w));
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
        let w = World::new();
        let r = run(&w, 30);
        let mut evidence = convinced(&r, &main_sha(&w));
        evidence.pull.as_mut().unwrap().base.name = "trunk".to_owned();

        let (_, concluded) = conducted(&r, &w, &evidence, &completing());

        assert!(
            report(&concluded).contains("trunk, not main"),
            "{concluded:?}"
        );
    }

    #[test]
    fn a_spent_run_is_failed_and_kept_for_forensics() {
        let w = World::new();
        let mut r = run(&w, 10);
        r.budget.max_tokens = Some(10);
        let agent = Scripted::playing(vec![
            Play::Usage(Usage::tokens(90, 20)),
            Play::Finish(Stop::Completed),
        ]);

        let (_, concluded) = conducted(&r, &w, &convinced(&r, &main_sha(&w)), &agent);

        assert!(report(&concluded).contains("budget"), "{concluded:?}");
        assert!(concluded.retained.is_some() && w.git.worktree(&w.repo, 10).is_dir());
    }

    #[test]
    fn a_run_that_cannot_get_a_worktree_fails_before_implementing() {
        let w = World::new();
        let mut r = run(&w, 3);
        r.url = w.root.path().join("no-such-origin").display().to_string();

        let (events, concluded) = conducted(&r, &w, &convinced(&r, &main_sha(&w)), &completing());

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
        let w = World::new();
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
