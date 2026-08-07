//! The old `epik:feature` loop, as a coded state machine.
//!
//! One [`FeatureRun`] reads the feature's sub-issue graph through the
//! [`Machinery`] seam, schedules it with [`ready`] — a pure fold, blocked-by
//! edges in, ready set out — and conducts an [`IssueRun`] for each ready
//! sub-issue against the feature branch until every sub-issue is closed.
//! Rounds are sequential here; spawning a round's ready set concurrently up
//! to a cap is the worker's affair, over the same fold. Edges encode
//! implementation dependencies only — there is no contention story, because
//! each implementer's definition of done includes resolving its own merge
//! conflicts and ending green.
//!
//! The endpoint is machinery from day one, the #70 lesson generalized: when
//! the graph is done, the run opens the review pull request from the feature
//! branch into the base — titled for the feature issue, closing it on merge,
//! linked rather than duplicated when one already stands — and never merges
//! it. Merging the review is the human's act, and the seam has no verb for
//! it. The verdict names that pull request as where to review.

use std::time::Duration;

use crate::agent::{Budget, CodingAgent};
use crate::chat::StopToken;
use crate::git::Git;
use crate::github::{self, GitHub, Issue, IssueGraph, Pull, Repo, State};
use crate::logging::Log;
use crate::run::{
    Evidence, FeatureEvent, FeaturePhase, FeatureVerdict, IssueRun, RunEvent, Verdict,
};

/// The scheduling fold: blocked-by edges in, ready set out.
///
/// A sub-issue is ready when it is open and everything it waits on is
/// closed — readiness is a property of the graph alone, so chains,
/// diamonds, and independent sets all fall out of the one rule. Order is
/// the graph's own, which keeps a round deterministic.
#[must_use]
pub fn ready(graphs: &[IssueGraph]) -> Vec<&IssueGraph> {
    graphs
        .iter()
        .filter(|graph| {
            graph.issue.state == State::Open
                && graph
                    .blocked_by
                    .iter()
                    .all(|edge| edge.state == State::Closed)
        })
        .collect()
}

/// What a feature run needs from GitHub beyond an issue run's [`Evidence`]:
/// the graph read, and the one mechanical act — opening the review pull
/// request.
///
/// [`GitHub`] in production, a double in tests, so the state machine stays
/// testable with no network. Judgment still lives in the run; deliberately,
/// the seam has no verb for merging, so no feature run can merge what it
/// opened.
pub trait Machinery: Evidence {
    /// One issue and the edges around it, read back.
    ///
    /// # Errors
    ///
    /// Returns a [`github::Error`] when GitHub cannot be asked or answers
    /// no.
    fn graph(&self, repo: &Repo, number: u64) -> Result<IssueGraph, github::Error>;

    /// Opens the pull request merging `head` into `base`.
    ///
    /// # Errors
    ///
    /// Returns a [`github::Error`] when GitHub cannot be asked or answers
    /// no.
    fn open_pull(
        &self,
        repo: &Repo,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
    ) -> Result<Pull, github::Error>;
}

impl Machinery for GitHub {
    fn graph(&self, repo: &Repo, number: u64) -> Result<IssueGraph, github::Error> {
        self.issue_graph(repo, number)
    }

    fn open_pull(
        &self,
        repo: &Repo,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
    ) -> Result<Pull, github::Error> {
        // The inherent verb: inherent methods win the name over this trait's.
        Self::open_pull(self, repo, title, head, base, body)
    }
}

/// One feature, run to a verdict: the fully provisioned input to
/// [`conduct`](Self::conduct), on the [`IssueRun`] precedent — everything
/// handed in, nothing discovered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureRun {
    pub repo: Repo,
    /// Where the repository is cloned from.
    pub url: String,
    /// The branch the review pull request merges into.
    pub base: String,
    /// The feature issue: its graph names the sub-issues, and the review
    /// pull request is titled for it.
    pub issue: Issue,
    /// Credentials injected, never discovered — passed through to every
    /// issue run.
    pub env: Vec<(String, String)>,
    /// What each issue run may spend.
    pub budget: Budget,
    /// How long each issue run's judgment waits for a check still running.
    pub patience: Duration,
}

impl FeatureRun {
    /// The feature branch: what every sub-issue merges into, and the review
    /// pull request's head. Deterministic, on the [`IssueRun::branch`]
    /// precedent.
    #[must_use]
    pub fn branch(&self) -> String {
        format!("feature-{}", self.issue.number)
    }

    /// Runs the whole feature to a verdict, narrating every state into
    /// `log`.
    ///
    /// Infallible by design, like [`IssueRun::conduct`]: everything that
    /// goes wrong — the seam refusing, an issue run failing, a graph with
    /// nothing ready — is a [`FeatureVerdict::Failed`] with a report, never
    /// an error the caller must route separately.
    pub fn conduct(
        &self,
        git: &Git,
        machinery: &dyn Machinery,
        agent: &dyn CodingAgent,
        log: &mut dyn Log<FeatureEvent>,
        stop: &StopToken,
    ) -> FeatureVerdict {
        let verdict = match self.driven(git, machinery, agent, log, stop) {
            Ok(review) => FeatureVerdict::Done { review },
            Err(report) => FeatureVerdict::Failed { report },
        };
        log.emit(FeatureEvent::Finished(verdict.clone()));
        verdict
    }

    /// Graph through review: read the sub-issues, drain the graph round by
    /// round, then reach the endpoint. What comes back on success is the
    /// review pull request's number — where to review.
    fn driven(
        &self,
        git: &Git,
        machinery: &dyn Machinery,
        agent: &dyn CodingAgent,
        log: &mut dyn Log<FeatureEvent>,
        stop: &StopToken,
    ) -> Result<u64, String> {
        log.emit(FeatureEvent::Entered(FeaturePhase::Graph));
        let feature = self.graph(machinery, self.issue.number)?;
        let numbers: Vec<u64> = feature.sub_issues.iter().map(|edge| edge.number).collect();
        let mut open = usize::MAX;
        loop {
            let graphs: Vec<IssueGraph> = numbers
                .iter()
                .map(|&number| self.graph(machinery, number))
                .collect::<Result<_, _>>()?;
            let remaining = graphs
                .iter()
                .filter(|graph| graph.issue.state == State::Open)
                .count();
            if remaining == 0 {
                break;
            }
            // Every run in a round was verified Done — issue closed, per the
            // seam's own facts — so a re-read that closes nothing means the
            // seam contradicts itself. Reported rather than spun on: the
            // loop's totality does not rest on the seam's honesty.
            if remaining >= open {
                return Err(format!(
                    "a scheduling round closed nothing: {remaining} sub-issues still open"
                ));
            }
            open = remaining;
            log.emit(FeatureEvent::Entered(FeaturePhase::Schedule));
            let round = ready(&graphs);
            if round.is_empty() {
                return Err(stuck(&graphs));
            }
            for graph in round {
                self.conducted(graph.issue.clone(), git, machinery, agent, log, stop)?;
            }
        }
        log.emit(FeatureEvent::Entered(FeaturePhase::Review));
        self.reviewed(machinery)
    }

    fn graph(&self, machinery: &dyn Machinery, number: u64) -> Result<IssueGraph, String> {
        machinery
            .graph(&self.repo, number)
            .map_err(|error| error.to_string())
    }

    /// One sub-issue, run to its verdict inside this feature: the work
    /// merges into the feature branch, and the whole run narrates through
    /// the feature's log stamped with its number.
    fn conducted(
        &self,
        issue: Issue,
        git: &Git,
        machinery: &dyn Machinery,
        agent: &dyn CodingAgent,
        log: &mut dyn Log<FeatureEvent>,
        stop: &StopToken,
    ) -> Result<(), String> {
        let number = issue.number;
        let run = IssueRun {
            repo: self.repo.clone(),
            url: self.url.clone(),
            base: self.branch(),
            issue,
            env: self.env.clone(),
            budget: self.budget,
            patience: self.patience,
        };
        let mut stamped = Stamped { number, log };
        let concluded = run.conduct(git, machinery, agent, &mut stamped, stop);
        match concluded.verdict {
            Verdict::Done => Ok(()),
            Verdict::Failed { report } => Err(format!("issue #{number}: {report}")),
        }
    }

    /// The endpoint, as machinery: the review pull request from the feature
    /// branch into the base, titled for the feature issue, closing it on
    /// merge. A pull request already standing on the feature branch is
    /// linked, never duplicated — and nothing here merges anything.
    fn reviewed(&self, machinery: &dyn Machinery) -> Result<u64, String> {
        let head = self.branch();
        let standing = machinery
            .pull(&self.repo, &head)
            .map_err(|error| error.to_string())?;
        if let Some(pull) = standing {
            return Ok(pull.number);
        }
        let pull = machinery
            .open_pull(
                &self.repo,
                &self.issue.title,
                &head,
                &self.base,
                &format!("Closes #{}", self.issue.number),
            )
            .map_err(|error| error.to_string())?;
        Ok(pull.number)
    }
}

/// The report for a round with nothing ready: every open sub-issue names
/// what it waits on, which is how a cycle — or a blocker outside the
/// feature that never closes — surfaces.
fn stuck(graphs: &[IssueGraph]) -> String {
    let waits: Vec<String> = graphs
        .iter()
        .filter(|graph| graph.issue.state == State::Open)
        .map(|graph| {
            let blockers: Vec<String> = graph
                .blocked_by
                .iter()
                .filter(|edge| edge.state == State::Open)
                .map(|edge| format!("#{}", edge.number))
                .collect();
            format!("#{} waits on {}", graph.issue.number, blockers.join(", "))
        })
        .collect();
    format!("no sub-issue is ready: {}", waits.join("; "))
}

/// A feature log lent to one issue run: every [`RunEvent`] rides out
/// stamped with the issue it belongs to.
struct Stamped<'a> {
    number: u64,
    log: &'a mut dyn Log<FeatureEvent>,
}

impl Log<RunEvent> for Stamped<'_> {
    fn emit(&mut self, event: RunEvent) {
        self.log.emit(FeatureEvent::Issue {
            number: self.number,
            event,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::agent::{Play, Scripted, Stop};
    use crate::git::testing::{World, rev, sh};
    use crate::github::{Branch, Check, Conclusion, Edge};

    fn issue(number: u64, state: State) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: "the details".to_owned(),
            state,
        }
    }

    fn node(number: u64, state: State, blocked_by: &[(u64, State)]) -> IssueGraph {
        IssueGraph {
            issue: issue(number, state),
            sub_issues: Vec::new(),
            blocked_by: blocked_by
                .iter()
                .map(|&(number, state)| Edge {
                    number,
                    title: format!("issue {number}"),
                    state,
                })
                .collect(),
        }
    }

    fn numbers(round: &[&IssueGraph]) -> Vec<u64> {
        round.iter().map(|graph| graph.issue.number).collect()
    }

    use State::{Closed, Open};

    #[test]
    fn an_independent_set_is_ready_all_at_once() {
        let graphs = [node(1, Open, &[]), node(2, Open, &[]), node(3, Open, &[])];
        assert_eq!(numbers(&ready(&graphs)), [1, 2, 3]);
    }

    #[test]
    fn a_chain_is_ready_one_link_at_a_time() {
        let fresh = [
            node(1, Open, &[]),
            node(2, Open, &[(1, Open)]),
            node(3, Open, &[(2, Open)]),
        ];
        assert_eq!(numbers(&ready(&fresh)), [1], "only the head of the chain");

        let after_head = [
            node(1, Closed, &[]),
            node(2, Open, &[(1, Closed)]),
            node(3, Open, &[(2, Open)]),
        ];
        assert_eq!(
            numbers(&ready(&after_head)),
            [2],
            "closing a link readies exactly the next one"
        );
    }

    #[test]
    fn a_diamond_forks_after_its_head_and_joins_on_both_arms() {
        let arms = |one, two, three| {
            [
                node(1, one, &[]),
                node(2, two, &[(1, one)]),
                node(3, three, &[(1, one)]),
                node(4, Open, &[(2, two), (3, three)]),
            ]
        };
        assert_eq!(numbers(&ready(&arms(Open, Open, Open))), [1]);
        assert_eq!(
            numbers(&ready(&arms(Closed, Open, Open))),
            [2, 3],
            "the head's closing forks the diamond"
        );
        assert_eq!(
            numbers(&ready(&arms(Closed, Closed, Open))),
            [3],
            "the join waits for both arms"
        );
        assert_eq!(numbers(&ready(&arms(Closed, Closed, Closed))), [4]);
    }

    #[test]
    fn a_closed_issue_is_never_ready_and_only_open_blockers_block() {
        let graphs = [
            node(1, Closed, &[]),
            node(2, Open, &[(99, Closed)]),
            node(3, Open, &[(99, Open)]),
        ];
        assert_eq!(
            numbers(&ready(&graphs)),
            [2],
            "a blocker outside the feature counts by its own state"
        );
    }

    /// The little GitHub a feature run is judged against: sub-issues with
    /// their blockers, playing a world where every conducted run's agent
    /// did its whole job — the first time judgment asks for an issue's pull
    /// request, the pull is there, merged into the feature branch, and the
    /// issue closes.
    struct Fake {
        feature: Issue,
        nodes: RefCell<Vec<Node>>,
        /// What every issue pull stands on: the feature branch's tip, which
        /// is where a scripted agent's worktree stands too.
        sha: String,
        branch: String,
        /// The pull request standing on the feature branch, when one does.
        review: RefCell<Option<Pull>>,
        /// Every `open_pull` this fake was asked for: (title, head, base,
        /// body).
        opened: RefCell<Vec<(String, String, String, String)>>,
        /// When set, the graph read never sees a closure: the
        /// self-contradicting seam the no-progress guard exists for.
        graph_denies_progress: bool,
    }

    struct Node {
        number: u64,
        blocked_by: Vec<u64>,
        closed: bool,
    }

    impl Fake {
        fn new(feature: Issue, branch: &str, sha: &str, nodes: &[(u64, &[u64], State)]) -> Self {
            Self {
                feature,
                nodes: RefCell::new(
                    nodes
                        .iter()
                        .map(|&(number, blocked_by, state)| Node {
                            number,
                            blocked_by: blocked_by.to_vec(),
                            closed: state == Closed,
                        })
                        .collect(),
                ),
                sha: sha.to_owned(),
                branch: branch.to_owned(),
                review: RefCell::new(None),
                opened: RefCell::new(Vec::new()),
                graph_denies_progress: false,
            }
        }

        fn edge(&self, number: u64) -> Edge {
            let nodes = self.nodes.borrow();
            let node = nodes
                .iter()
                .find(|node| node.number == number)
                .expect("an edge points at a node this fake knows");
            Edge {
                number,
                title: format!("issue {number}"),
                state: if node.closed && !self.graph_denies_progress {
                    Closed
                } else {
                    Open
                },
            }
        }
    }

    impl Evidence for Fake {
        fn pull(&self, _: &Repo, head: &str) -> Result<Option<Pull>, github::Error> {
            if head == self.branch {
                return Ok(self.review.borrow().clone());
            }
            let mut nodes = self.nodes.borrow_mut();
            let node = nodes
                .iter_mut()
                .find(|node| format!("issue-{}", node.number) == head)
                .expect("judgment asks only about branches this fake knows");
            node.closed = true;
            Ok(Some(Pull {
                number: 100 + node.number,
                title: format!("issue {}", node.number),
                state: Closed,
                merged: true,
                head: Branch {
                    name: head.to_owned(),
                    sha: self.sha.clone(),
                },
                base: Branch {
                    name: self.branch.clone(),
                    sha: self.sha.clone(),
                },
            }))
        }

        fn checks(&self, _: &Repo, _: &str) -> Result<Vec<Check>, github::Error> {
            Ok(vec![Check {
                name: "CI".to_owned(),
                conclusion: Some(Conclusion::Success),
            }])
        }

        fn issue(&self, _: &Repo, number: u64) -> Result<Issue, github::Error> {
            let nodes = self.nodes.borrow();
            let node = nodes
                .iter()
                .find(|node| node.number == number)
                .expect("judgment asks only about issues this fake knows");
            Ok(issue(number, if node.closed { Closed } else { Open }))
        }
    }

    impl Machinery for Fake {
        fn graph(&self, _: &Repo, number: u64) -> Result<IssueGraph, github::Error> {
            if number == self.feature.number {
                let nodes = self.nodes.borrow();
                return Ok(IssueGraph {
                    issue: self.feature.clone(),
                    sub_issues: nodes.iter().map(|node| self.edge(node.number)).collect(),
                    blocked_by: Vec::new(),
                });
            }
            let nodes = self.nodes.borrow();
            let blocked_by = &nodes
                .iter()
                .find(|node| node.number == number)
                .expect("the run asks only about sub-issues the feature named")
                .blocked_by;
            Ok(IssueGraph {
                issue: issue(number, self.edge(number).state),
                sub_issues: Vec::new(),
                blocked_by: blocked_by.iter().map(|&number| self.edge(number)).collect(),
            })
        }

        fn open_pull(
            &self,
            _: &Repo,
            title: &str,
            head: &str,
            base: &str,
            body: &str,
        ) -> Result<Pull, github::Error> {
            self.opened.borrow_mut().push((
                title.to_owned(),
                head.to_owned(),
                base.to_owned(),
                body.to_owned(),
            ));
            let pull = Pull {
                number: 500,
                title: title.to_owned(),
                state: Open,
                merged: false,
                head: Branch {
                    name: head.to_owned(),
                    sha: self.sha.clone(),
                },
                base: Branch {
                    name: base.to_owned(),
                    sha: self.sha.clone(),
                },
            };
            *self.review.borrow_mut() = Some(pull.clone());
            Ok(pull)
        }
    }

    fn run(world: &World, number: u64) -> FeatureRun {
        FeatureRun {
            repo: world.repo.clone(),
            url: world.url(),
            base: "main".to_owned(),
            issue: issue(number, Open),
            env: Vec::new(),
            budget: Budget {
                max_tokens: None,
                max_cost: None,
                stall: Duration::from_mins(1),
            },
            patience: Duration::ZERO,
        }
    }

    /// A [`World`] whose origin also carries the feature branch, plus the
    /// tip every convinced pull stands on.
    fn world_with(branch: &str) -> (World, String) {
        let w = World::new();
        sh(&w.origin, &["branch", branch]);
        let sha = rev(&w.origin, branch);
        (w, sha)
    }

    fn completing() -> Scripted {
        Scripted::playing(vec![
            Play::Progress("implementing".to_owned()),
            Play::Finish(Stop::Completed),
        ])
    }

    fn conducted(
        run: &FeatureRun,
        world: &World,
        fake: &Fake,
        agent: &dyn CodingAgent,
    ) -> (Vec<FeatureEvent>, FeatureVerdict) {
        let mut events: Vec<FeatureEvent> = Vec::new();
        let verdict = run.conduct(&world.git, fake, agent, &mut events, &StopToken::new());
        assert_eq!(
            events.last(),
            Some(&FeatureEvent::Finished(verdict.clone())),
            "the log's last word and the returned verdict tell one story"
        );
        (events, verdict)
    }

    fn phases(events: &[FeatureEvent]) -> Vec<FeaturePhase> {
        events
            .iter()
            .filter_map(|event| match event {
                FeatureEvent::Entered(phase) => Some(*phase),
                _ => None,
            })
            .collect()
    }

    fn report(verdict: &FeatureVerdict) -> &str {
        match verdict {
            FeatureVerdict::Failed { report } => report,
            FeatureVerdict::Done { .. } => panic!("expected a failed run: {verdict:?}"),
        }
    }

    #[test]
    fn a_full_run_drains_the_graph_in_dependency_order_and_opens_the_review_pull() {
        let r_branch = "feature-104";
        let (w, sha) = world_with(r_branch);
        let r = run(&w, 104);
        assert_eq!(r.branch(), r_branch);
        // A chain 1 → 2 beside an independent 3: two scheduling rounds.
        let fake = Fake::new(
            r.issue.clone(),
            r_branch,
            &sha,
            &[(1, &[], Open), (2, &[1], Open), (3, &[], Open)],
        );

        let (events, verdict) = conducted(&r, &w, &fake, &completing());

        assert_eq!(verdict, FeatureVerdict::Done { review: 500 });
        assert_eq!(
            phases(&events),
            [
                FeaturePhase::Graph,
                FeaturePhase::Schedule,
                FeaturePhase::Schedule,
                FeaturePhase::Review,
            ],
            "the graph drained in two rounds"
        );
        let starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event {
                FeatureEvent::Issue {
                    number,
                    event: RunEvent::Entered(crate::run::Phase::Worktree),
                } => Some(*number),
                _ => None,
            })
            .collect();
        assert_eq!(starts, [1, 3, 2], "the blocked issue ran last");
        let opened = fake.opened.borrow();
        let [(title, head, base, body)] = opened.as_slice() else {
            panic!("exactly one review pull request: {opened:?}");
        };
        assert_eq!(title, &r.issue.title, "titled for the feature issue");
        assert_eq!(head, r_branch);
        assert_eq!(base, "main");
        assert!(body.contains("Closes #104"), "{body}");
        assert!(
            !fake.review.borrow().as_ref().unwrap().merged,
            "the machinery opens the review and never merges it"
        );
        for number in [1, 2, 3] {
            assert!(
                !w.git.worktree(&w.repo, number).exists(),
                "each verified issue run cleaned its worktree"
            );
        }
    }

    #[test]
    fn an_existing_review_pull_is_linked_never_duplicated() {
        let (w, sha) = world_with("feature-105");
        let r = run(&w, 105);
        let fake = Fake::new(r.issue.clone(), "feature-105", &sha, &[(1, &[], Closed)]);
        *fake.review.borrow_mut() = Some(Pull {
            number: 77,
            title: "already standing".to_owned(),
            state: Open,
            merged: false,
            head: Branch {
                name: "feature-105".to_owned(),
                sha: sha.clone(),
            },
            base: Branch {
                name: "main".to_owned(),
                sha,
            },
        });

        let (events, verdict) = conducted(&r, &w, &fake, &completing());

        assert_eq!(
            verdict,
            FeatureVerdict::Done { review: 77 },
            "the standing pull request is the one named for review"
        );
        assert!(fake.opened.borrow().is_empty(), "linked, not duplicated");
        assert_eq!(
            phases(&events),
            [FeaturePhase::Graph, FeaturePhase::Review],
            "sub-issues already closed schedule no round"
        );
    }

    #[test]
    fn a_cycle_fails_loudly_naming_the_waits() {
        let (w, sha) = world_with("feature-106");
        let r = run(&w, 106);
        let fake = Fake::new(
            r.issue.clone(),
            "feature-106",
            &sha,
            &[(1, &[2], Open), (2, &[1], Open)],
        );

        let (events, verdict) = conducted(&r, &w, &fake, &completing());

        let report = report(&verdict);
        assert!(report.contains("no sub-issue is ready"), "{report}");
        assert!(report.contains("#1 waits on #2"), "{report}");
        assert!(report.contains("#2 waits on #1"), "{report}");
        assert!(
            !phases(&events).contains(&FeaturePhase::Review),
            "a stuck graph never reaches the endpoint"
        );
        assert!(fake.opened.borrow().is_empty());
    }

    #[test]
    fn a_failed_issue_run_fails_the_feature_naming_the_issue() {
        let (w, sha) = world_with("feature-107");
        let r = run(&w, 107);
        let fake = Fake::new(r.issue.clone(), "feature-107", &sha, &[(1, &[], Open)]);
        let blocked = Scripted::playing(vec![Play::Finish(Stop::Blocked {
            report: "the issue contradicts itself".to_owned(),
        })]);

        let (events, verdict) = conducted(&r, &w, &fake, &blocked);

        let report = report(&verdict);
        assert!(report.contains("issue #1"), "{report}");
        assert!(report.contains("the issue contradicts itself"), "{report}");
        assert!(
            fake.opened.borrow().is_empty(),
            "no review pull request for an unfinished feature"
        );
        assert!(
            !phases(&events).contains(&FeaturePhase::Review),
            "a failed round never reaches the endpoint"
        );
        assert!(
            w.git.worktree(&w.repo, 1).is_dir(),
            "the failed issue run's forensics are retained, feature or no feature"
        );
    }

    #[test]
    fn a_seam_that_contradicts_itself_is_reported_rather_than_spun_on() {
        let (w, sha) = world_with("feature-108");
        let r = run(&w, 108);
        let mut fake = Fake::new(r.issue.clone(), "feature-108", &sha, &[(1, &[], Open)]);
        // The issue read says closed after the run; the graph read denies
        // it. A second round would rerun the same issue forever.
        fake.graph_denies_progress = true;

        let (_, verdict) = conducted(&r, &w, &fake, &completing());

        assert!(report(&verdict).contains("closed nothing"), "{verdict:?}");
    }
}
