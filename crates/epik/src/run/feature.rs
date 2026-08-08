//! The old `epik:feature` loop, as a coded state machine.
//!
//! One [`FeatureRun`] reads the feature's sub-issue graph through the
//! [`Machinery`] seam, ensures the feature branch exists at the remote, and
//! schedules the graph with [`ready`] — a pure fold, blocked-by edges in,
//! ready set out — conducting an [`IssueRun`] for each ready sub-issue
//! against the feature branch until every sub-issue is closed. Rounds are
//! sequential here; spawning a round's ready set concurrently up to a cap is
//! the worker's affair, over the same fold. Edges encode implementation
//! dependencies only — there is no contention story, because each
//! implementer's definition of done includes resolving its own merge
//! conflicts and ending green.
//!
//! The endpoint is machinery from day one, the #70 lesson generalized: when
//! the graph is done, the run opens the review pull request from the feature
//! branch into the base — titled for the feature issue, closing it on merge,
//! linked rather than duplicated when one already stands — and never merges
//! it. Merging the review is the human's act, and the seam has no verb for
//! it. The verdict names that pull request as where to review.

use std::collections::BTreeSet;
use std::time::Duration;

use crate::agent::{Budget, CodingAgent};
use crate::chat::StopToken;
use crate::git::Git;
use crate::github::{self, GitHub, Issue, IssueGraph, Pull, Repo, State};
use crate::logging::{Log, Mapping};
use crate::run::{
    Evidence, FeatureEvent, FeaturePhase, FeatureVerdict, IssueRun, Retained, RunEvent, Verdict,
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
/// the graph and branch reads, and the mechanical acts — creating the
/// feature branch, opening the review pull request.
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

    /// The commit `branch` stands on at the remote, or `None` when there is
    /// no such branch.
    ///
    /// # Errors
    ///
    /// Returns a [`github::Error`] when GitHub cannot be asked or answers
    /// no.
    fn branch(&self, repo: &Repo, branch: &str) -> Result<Option<String>, github::Error>;

    /// Creates `branch` at `sha` at the remote.
    ///
    /// # Errors
    ///
    /// Returns a [`github::Error`] when GitHub cannot be asked or answers
    /// no.
    fn create_branch(&self, repo: &Repo, branch: &str, sha: &str) -> Result<(), github::Error>;

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

    fn branch(&self, repo: &Repo, branch: &str) -> Result<Option<String>, github::Error> {
        self.branch_sha(repo, branch)
    }

    fn create_branch(&self, repo: &Repo, branch: &str, sha: &str) -> Result<(), github::Error> {
        // The inherent verb: inherent methods win the name over this trait's.
        Self::create_branch(self, repo, branch, sha)
    }

    fn open_pull(
        &self,
        repo: &Repo,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
    ) -> Result<Pull, github::Error> {
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

    /// Graph through review: read the sub-issues, provision the feature
    /// branch, drain the graph round by round, then reach the endpoint.
    /// What comes back on success is the review pull request's number —
    /// where to review.
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
        if feature.sub_issues.is_empty() {
            return Err(format!("feature #{} has no sub-issues", self.issue.number));
        }
        log.emit(FeatureEvent::Entered(FeaturePhase::Branch));
        self.branched(machinery)?;
        // The sub-issues this run itself verified closed, kept beside the
        // seam's picture: closure was judged through REST reads and the
        // graph is a GraphQL read, so the picture may lag the fact. Open
        // and ready consult both, which is what keeps a lagging read from
        // stalling a round or rerunning a finished issue — and what makes
        // the loop total: every round either errs or grows this set.
        let mut closed: BTreeSet<u64> = BTreeSet::new();
        let mut edges = feature.sub_issues;
        loop {
            let open: Vec<u64> = edges
                .iter()
                .filter(|edge| edge.state == State::Open && !closed.contains(&edge.number))
                .map(|edge| edge.number)
                .collect();
            if open.is_empty() {
                break;
            }
            log.emit(FeatureEvent::Entered(FeaturePhase::Schedule));
            let mut graphs: Vec<IssueGraph> = open
                .iter()
                .map(|&number| self.graph(machinery, number))
                .collect::<Result<_, _>>()?;
            for graph in &mut graphs {
                for edge in &mut graph.blocked_by {
                    if closed.contains(&edge.number) {
                        edge.state = State::Closed;
                    }
                }
            }
            let round = ready(&graphs);
            if round.is_empty() {
                return Err(stuck(&graphs));
            }
            for graph in round {
                self.conducted(graph.issue.clone(), git, machinery, agent, log, stop)?;
                closed.insert(graph.issue.number);
            }
            // One re-read per round, for what this run cannot know on its
            // own: a sub-issue closed — or added — from outside.
            edges = self.graph(machinery, self.issue.number)?.sub_issues;
        }
        log.emit(FeatureEvent::Entered(FeaturePhase::Review));
        self.reviewed(machinery)
    }

    fn graph(&self, machinery: &dyn Machinery, number: u64) -> Result<IssueGraph, String> {
        machinery
            .graph(&self.repo, number)
            .map_err(|error| error.to_string())
    }

    /// Ensures the feature branch exists at the remote — cut from the base
    /// branch's tip when absent, so the first issue run has an
    /// `origin/<branch>` to check out from. Mechanical, so it is the
    /// harness's own act, never an agent's errand.
    fn branched(&self, machinery: &dyn Machinery) -> Result<(), String> {
        let head = self.branch();
        let standing = machinery
            .branch(&self.repo, &head)
            .map_err(|error| error.to_string())?;
        if standing.is_some() {
            return Ok(());
        }
        let base = machinery
            .branch(&self.repo, &self.base)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("base branch {} does not exist", self.base))?;
        machinery
            .create_branch(&self.repo, &head, &base)
            .map_err(|error| error.to_string())
    }

    /// One sub-issue, run to its verdict inside this feature: the work
    /// merges into the feature branch, and the whole run narrates through
    /// the feature's log stamped with its number. A worktree the run kept
    /// is narrated too, and a failure's report names where the forensics
    /// are.
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
        let mut stamped = Mapping::new(&mut *log, move |event: RunEvent| FeatureEvent::Issue {
            number,
            event,
        });
        let concluded = run.conduct(git, machinery, agent, &mut stamped, stop);
        let kept = concluded.retained.as_ref().map(|retained| match retained {
            Retained::Forensics(worktree) => (worktree.path().to_owned(), None),
            Retained::Undeleted(worktree, error) => {
                (worktree.path().to_owned(), Some(error.to_string()))
            }
        });
        if let Some((path, error)) = &kept {
            log.emit(FeatureEvent::Retained {
                number,
                path: path.clone(),
                error: error.clone(),
            });
        }
        match concluded.verdict {
            Verdict::Done => Ok(()),
            Verdict::Failed { report } => Err(match kept {
                Some((path, _)) => format!(
                    "issue #{number}: {report}; worktree retained at {}",
                    path.display()
                ),
                None => format!("issue #{number}: {report}"),
            }),
        }
    }

    /// The endpoint, as machinery: the review pull request from the feature
    /// branch into the base, titled for the feature issue, closing it on
    /// merge — and nothing here merges anything.
    fn reviewed(&self, machinery: &dyn Machinery) -> Result<u64, String> {
        let head = self.branch();
        // Standing: open, or already merged — a rerun of a feature whose
        // review merged links where the review happened. Closed without
        // merging is a rejected review with nothing left to say, so a
        // fresh one is opened rather than pointing at the dead one.
        let standing = machinery
            .pull(&self.repo, &head)
            .map_err(|error| error.to_string())?
            .filter(|pull| pull.state == State::Open || pull.merged);
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

/// The report for a round with nothing ready. Every graph handed in is an
/// open sub-issue — the round's own subset — so each names what it waits
/// on, which is how a cycle, or a blocker outside the feature that never
/// closes, surfaces.
fn stuck(graphs: &[IssueGraph]) -> String {
    let waits: Vec<String> = graphs
        .iter()
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;
    use crate::agent::{Play, Scripted, Stop};
    use crate::git::testing::{World, commit, rev, sh};
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

    /// The commit `name` stands on at the origin, or `None` when there is
    /// no such branch — the probe the fake's branch verb and the tests'
    /// assertions share.
    fn origin_branch(origin: &Path, name: &str) -> Option<String> {
        let output = Command::new("git")
            .current_dir(origin)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ])
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// The little GitHub a feature run is judged against: sub-issues with
    /// their blockers, playing a world where every conducted run's agent
    /// did its whole job — the first time judgment asks for an issue's pull
    /// request, the pull is there, merged into the feature branch, and the
    /// issue closes.
    ///
    /// Its branch verbs read and write the [`World`]'s origin, exactly the
    /// rendezvous production has: a branch the fake creates is a branch the
    /// issue runs' fetches see.
    struct Fake {
        feature: Issue,
        nodes: RefCell<Vec<Node>>,
        /// What every issue pull stands on: the base tip, which is where a
        /// scripted agent's worktree stands too.
        sha: String,
        branch: String,
        origin: PathBuf,
        /// The pull request standing on the feature branch, when one does.
        review: RefCell<Option<Pull>>,
        /// Every `open_pull` this fake was asked for: (title, head, base,
        /// body).
        opened: RefCell<Vec<(String, String, String, String)>>,
        /// When set, the graph read never reflects a closure: the REST-
        /// verified fact lagging the GraphQL picture, exaggerated to
        /// forever.
        graph_lags: bool,
    }

    struct Node {
        number: u64,
        blocked_by: Vec<u64>,
        closed: bool,
    }

    impl Fake {
        fn new(world: &World, feature: u64, nodes: &[(u64, &[u64], State)]) -> Self {
            Self {
                feature: issue(feature, Open),
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
                sha: rev(&world.origin, "main"),
                branch: format!("feature-{feature}"),
                origin: world.origin.clone(),
                review: RefCell::new(None),
                opened: RefCell::new(Vec::new()),
                graph_lags: false,
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
                state: if node.closed && !self.graph_lags {
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

        fn branch(&self, _: &Repo, branch: &str) -> Result<Option<String>, github::Error> {
            Ok(origin_branch(&self.origin, branch))
        }

        fn create_branch(&self, _: &Repo, branch: &str, sha: &str) -> Result<(), github::Error> {
            sh(&self.origin, &["branch", branch, sha]);
            Ok(())
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

    /// A review pull request on the feature branch, in the caller's stated
    /// state: what a previous run — or a human — left standing.
    fn review(number: u64, branch: &str, sha: &str, state: State, merged: bool) -> Pull {
        Pull {
            number,
            title: "the feature".to_owned(),
            state,
            merged,
            head: Branch {
                name: branch.to_owned(),
                sha: sha.to_owned(),
            },
            base: Branch {
                name: "main".to_owned(),
                sha: sha.to_owned(),
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

    /// The issues whose runs began, in order: each issue run opens by
    /// entering its worktree phase.
    fn starts(events: &[FeatureEvent]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|event| match event {
                FeatureEvent::Issue {
                    number,
                    event: RunEvent::Entered(crate::run::Phase::Worktree),
                } => Some(*number),
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
    fn a_full_run_creates_the_feature_branch_drains_the_graph_and_opens_the_review_pull() {
        let w = World::new();
        let r = run(&w, 104);
        // A chain 1 → 2 beside an independent 3: two scheduling rounds.
        let fake = Fake::new(&w, 104, &[(1, &[], Open), (2, &[1], Open), (3, &[], Open)]);
        assert_eq!(
            origin_branch(&w.origin, "feature-104"),
            None,
            "nothing pre-creates the feature branch: the run must"
        );

        let (events, verdict) = conducted(&r, &w, &fake, &completing());

        assert_eq!(verdict, FeatureVerdict::Done { review: 500 });
        assert_eq!(
            origin_branch(&w.origin, "feature-104").as_deref(),
            Some(fake.sha.as_str()),
            "the feature branch exists at origin, cut from the base tip"
        );
        assert_eq!(
            phases(&events),
            [
                FeaturePhase::Graph,
                FeaturePhase::Branch,
                FeaturePhase::Schedule,
                FeaturePhase::Schedule,
                FeaturePhase::Review,
            ],
            "the graph drained in two rounds"
        );
        assert_eq!(starts(&events), [1, 3, 2], "the blocked issue ran last");
        let opened = fake.opened.borrow();
        let [(title, head, base, body)] = opened.as_slice() else {
            panic!("exactly one review pull request: {opened:?}");
        };
        assert_eq!(title, &r.issue.title, "titled for the feature issue");
        assert_eq!(head, "feature-104");
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
    fn an_existing_feature_branch_is_reused_not_recreated() {
        let w = World::new();
        // The branch already stands one commit past the base: a recreation
        // would be refused, and a reset would discard merged sub-issue work.
        sh(&w.origin, &["checkout", "-b", "feature-109"]);
        commit(&w.origin, "landed.md", "a sub-issue already merged");
        sh(&w.origin, &["checkout", "main"]);
        let tip = origin_branch(&w.origin, "feature-109");
        assert_ne!(tip.as_deref(), Some(rev(&w.origin, "main").as_str()));
        let r = run(&w, 109);
        let mut fake = Fake::new(&w, 109, &[(1, &[], Open)]);
        // The issue pulls stand where the feature branch does.
        fake.sha = tip.clone().unwrap();

        let (_, verdict) = conducted(&r, &w, &fake, &completing());

        assert!(
            matches!(verdict, FeatureVerdict::Done { .. }),
            "{verdict:?}"
        );
        assert_eq!(
            origin_branch(&w.origin, "feature-109"),
            tip,
            "a standing feature branch is left exactly where it stood"
        );
    }

    #[test]
    fn an_open_review_pull_is_linked_never_duplicated() {
        let w = World::new();
        let r = run(&w, 105);
        let fake = Fake::new(&w, 105, &[(1, &[], Closed)]);
        *fake.review.borrow_mut() = Some(review(77, "feature-105", &fake.sha, Open, false));

        let (events, verdict) = conducted(&r, &w, &fake, &completing());

        assert_eq!(
            verdict,
            FeatureVerdict::Done { review: 77 },
            "the standing pull request is the one named for review"
        );
        assert!(fake.opened.borrow().is_empty(), "linked, not duplicated");
        assert_eq!(
            phases(&events),
            [
                FeaturePhase::Graph,
                FeaturePhase::Branch,
                FeaturePhase::Review
            ],
            "sub-issues already closed schedule no round"
        );
    }

    #[test]
    fn a_rerun_of_a_reviewed_and_merged_feature_links_the_merged_review() {
        let w = World::new();
        let r = run(&w, 110);
        let fake = Fake::new(&w, 110, &[(1, &[], Closed)]);
        *fake.review.borrow_mut() = Some(review(78, "feature-110", &fake.sha, Closed, true));

        let (_, verdict) = conducted(&r, &w, &fake, &completing());

        assert_eq!(
            verdict,
            FeatureVerdict::Done { review: 78 },
            "the feature is already done, and the merged review is where it happened"
        );
        assert!(fake.opened.borrow().is_empty());
    }

    #[test]
    fn a_review_pull_closed_without_merging_is_dead_and_a_fresh_one_is_opened() {
        let w = World::new();
        let r = run(&w, 111);
        let fake = Fake::new(&w, 111, &[(1, &[], Closed)]);
        *fake.review.borrow_mut() = Some(review(79, "feature-111", &fake.sha, Closed, false));

        let (_, verdict) = conducted(&r, &w, &fake, &completing());

        assert_eq!(
            verdict,
            FeatureVerdict::Done { review: 500 },
            "a rejected review has nothing left to say"
        );
        assert_eq!(fake.opened.borrow().len(), 1, "a fresh pull request");
    }

    #[test]
    fn a_feature_with_no_sub_issues_fails_loudly_before_touching_anything() {
        let w = World::new();
        let r = run(&w, 112);
        let fake = Fake::new(&w, 112, &[]);

        let (events, verdict) = conducted(&r, &w, &fake, &completing());

        assert!(
            report(&verdict).contains("feature #112 has no sub-issues"),
            "{verdict:?}"
        );
        assert_eq!(
            origin_branch(&w.origin, "feature-112"),
            None,
            "an empty feature creates no branch"
        );
        assert!(fake.opened.borrow().is_empty(), "and opens no pull request");
        assert_eq!(phases(&events), [FeaturePhase::Graph]);
    }

    #[test]
    fn a_cycle_fails_loudly_naming_the_waits() {
        let w = World::new();
        let r = run(&w, 106);
        let fake = Fake::new(&w, 106, &[(1, &[2], Open), (2, &[1], Open)]);

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
    fn a_failed_issue_run_fails_the_feature_naming_the_issue_and_its_forensics() {
        let w = World::new();
        let r = run(&w, 107);
        let fake = Fake::new(&w, 107, &[(1, &[], Open)]);
        let blocked = Scripted::playing(vec![Play::Finish(Stop::Blocked {
            report: "the issue contradicts itself".to_owned(),
        })]);

        let (events, verdict) = conducted(&r, &w, &fake, &blocked);

        let forensics = w.git.worktree(&w.repo, 1);
        let report = report(&verdict);
        assert!(report.contains("issue #1"), "{report}");
        assert!(report.contains("the issue contradicts itself"), "{report}");
        assert!(
            report.contains(&format!("worktree retained at {}", forensics.display())),
            "the report says where the forensics are: {report}"
        );
        assert!(
            events.contains(&FeatureEvent::Retained {
                number: 1,
                path: forensics.clone(),
                error: None,
            }),
            "the retention is narrated too: {events:?}"
        );
        assert!(forensics.is_dir(), "and the worktree really is there");
        assert!(
            fake.opened.borrow().is_empty(),
            "no review pull request for an unfinished feature"
        );
        assert!(
            !phases(&events).contains(&FeaturePhase::Review),
            "a failed round never reaches the endpoint"
        );
    }

    #[test]
    fn a_graph_read_lagging_verified_closures_neither_stalls_nor_reruns() {
        let w = World::new();
        let r = run(&w, 108);
        let mut fake = Fake::new(&w, 108, &[(1, &[], Open), (2, &[1], Open)]);
        // The graph never reflects a closure, though each run's own REST
        // judgment verified it: read-after-write lag, exaggerated to
        // forever. The run's own knowledge must carry the round.
        fake.graph_lags = true;

        let (events, verdict) = conducted(&r, &w, &fake, &completing());

        assert_eq!(
            verdict,
            FeatureVerdict::Done { review: 500 },
            "verified closures count even when the graph read lags them"
        );
        assert_eq!(
            starts(&events),
            [1, 2],
            "each issue ran exactly once — no stall, no rerun"
        );
    }
}
