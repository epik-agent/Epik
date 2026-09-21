//! A feature build as one plan: the feature's branch, then its issues
//! in the order the tracker gives them, then its pull request.
//!
//! A [`Build`] is everything a feature is built with, and
//! [`Build::plan`] lays the three kinds of [`Node`] over the tracker's
//! [`Dag`] of issue numbers. Every node holds the one build, so every
//! process any of them launches goes through the one set of slots, and
//! everything any of them says goes into the one sink, under its
//! [`Name`].
//!
//! An [`Issue`] is a sequence of [`Step`]s, each awaited in order and
//! each with its lifecycle on the stream. The steps that run a process
//! hold a slot while it runs; the steps that talk to GitHub hold none,
//! so an issue waiting on its checks leaves its slot to another's
//! agent. The pushed commit and the pull request's number are threaded
//! from step to step inside the one node.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::commit::{Commit, NothingToCommit};
use super::github::{CheckWait, CreateBranch, Feature, OpenPull};
use super::push::Push;
use super::stream::{Event, Happened, Reported, Sink, reported};
use super::task::Task;
use super::{Implement, Plan, ProcessSlots};
use crate::agent::Agent;
use crate::dag::Dag;
use crate::git::askpass::Credentials;
use crate::git::commands::{Hash, RevParse};
use crate::git::url::Url;
use crate::git::{self, Repository};
use crate::github::Merge;
use crate::worktrees::{LinkedWorktree, Worktrees};

/// The branch issue `number` is implemented on.
#[must_use]
pub fn issue_branch(number: u64) -> String {
    format!("issue-{number}")
}

/// What a feature build's events are filed under: a node of its plan,
/// or one step of an issue's node.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Name {
    Branch,
    Issue(u64),
    Step(u64, Step),
    PullRequest,
}

/// The steps of an issue's node, in the order they run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Step {
    /// The issue's branch cut at the feature branch's tip, in a
    /// worktree of its own.
    Worktree,
    /// The agent's run there.
    Agent,
    /// What the agent left, committed as Epik.
    Commit,
    /// The issue's branch pushed.
    Push,
    /// The issue's pull request opened into the feature branch.
    PullRequest,
    /// The wait for the pushed commit's checks.
    Checks,
    /// The pull request merged.
    Merge,
    /// The issue closed.
    Close,
}

/// Everything one feature is built with. The token of the feature's
/// GitHub client is the build's one token: its calls to GitHub carry
/// it, and every git that talks to `url` answers with the
/// [credentials](crate::github::GitHub::credentials) made of it.
pub struct Build<A> {
    pub feature: Feature,
    /// Where the repository is cloned from and pushed to.
    pub url: Url,
    pub worktrees: Worktrees,
    pub checks: CheckWait,
    /// How an issue's pull request merges into the feature branch.
    pub merge: Merge,
    /// The slots every process of the build runs in.
    pub slots: ProcessSlots,
    pub sink: Sink<Event<Name>>,
    pub agent: Agents<A>,
}

/// The agent that implements the issue numbered, in the worktree at
/// the path: made as the issue's worktree is, which is when its
/// working directory is known.
pub type Agents<A> = Box<dyn Fn(u64, &Path) -> A + Send + Sync>;

impl<A> Build<A> {
    /// The plan that builds the feature: `work`, the feature's issues
    /// as its tracker orders them, between the feature's branch and
    /// its pull request.
    #[must_use]
    pub fn plan(self, work: Dag<u64>) -> Plan<Reported<Node<A>>> {
        let build = Arc::new(self);
        let node = |task: Node<A>| Reported {
            sink: build.sink.node(task.name()),
            task,
        };
        let issue = |number| {
            let build = Arc::clone(&build);
            node(Node::Issue(Issue { number, build }))
        };
        let feature = &build.feature;
        work.map(issue).between(
            node(Node::Branch(CreateBranch(feature.clone()))),
            node(Node::PullRequest(OpenPull(feature.clone()))),
        )
    }
}

/// A node of a feature's plan. The plan holds each one [`Reported`]
/// under its [`name`](Self::name).
pub enum Node<A> {
    /// The plan's source: the feature's branch, created.
    Branch(CreateBranch),
    /// One issue, implemented and merged into the feature branch.
    Issue(Issue<A>),
    /// The plan's sink: the feature's pull request, left open.
    PullRequest(OpenPull),
}

impl<A> Node<A> {
    /// What the node's events are filed under.
    #[must_use]
    pub const fn name(&self) -> Name {
        match self {
            Self::Branch(_) => Name::Branch,
            Self::Issue(issue) => Name::Issue(issue.number),
            Self::PullRequest(_) => Name::PullRequest,
        }
    }
}

impl<A: Agent + Clone + Send + Sync + 'static> Task for Node<A> {
    async fn run(&self) -> Result<()> {
        match self {
            Self::Branch(task) => task.run().await,
            Self::Issue(task) => task.run().await,
            Self::PullRequest(task) => task.run().await,
        }
    }
}

/// One issue of a feature, from its worktree to its closing.
pub struct Issue<A> {
    number: u64,
    build: Arc<Build<A>>,
}

impl<A: Agent + Clone + Send + Sync + 'static> Task for Issue<A> {
    /// The steps in order, to the first that fails, whose failure is
    /// the issue's.
    async fn run(&self) -> Result<()> {
        let Build {
            feature,
            checks,
            merge,
            ..
        } = &*self.build;
        let (number, branch) = (self.number, issue_branch(self.number));
        let pushed = self.pushed().await?.to_string();
        let opened = feature.open_issue_pull(number, &branch);
        let pull = self.step(Step::PullRequest, opened).await?;
        let verdict = feature.await_checks(&pushed, *checks);
        self.step(Step::Checks, verdict).await?;
        let merged = feature.merge(pull.number, *merge);
        self.step(Step::Merge, merged).await?;
        self.step(Step::Close, feature.close(number)).await
    }
}

impl<A: Agent + Clone + Send + Sync + 'static> Issue<A> {
    /// The steps that run processes, each in a slot for as long as its
    /// process runs: the commit the issue's branch was pushed at. The
    /// worktree stands until the push returns, and its removal is
    /// git's work too, so it goes through the door.
    async fn pushed(&self) -> Result<Hash> {
        let Build {
            feature,
            url,
            worktrees,
            slots,
            ..
        } = &*self.build;
        let (worktrees, url) = (worktrees.clone(), url.clone());
        let (branch, at) = (
            issue_branch(self.number),
            format!("origin/{}", feature.branch()),
        );
        let github = feature.github.clone();
        let added = slots.run(move || {
            let credentials = github.credentials()?;
            let linked = worktrees.add(&url, &branch, &at, &credentials)?;
            Ok((linked, credentials))
        });
        let (linked, credentials) = self.step(Step::Worktree, added).await?;
        let pushed = self.worked(&linked, credentials).await;
        slots
            .run(move || {
                drop(linked);
                Ok(())
            })
            .await?;
        pushed
    }

    /// The agent's run in `linked`, and what it left there committed
    /// and pushed as the holder of `credentials`: the commit pushed.
    async fn worked(&self, linked: &LinkedWorktree, credentials: Credentials) -> Result<Hash> {
        let Build { slots, agent, .. } = &*self.build;
        let worktree = linked.worktree();
        let implement = Implement {
            agent: agent(self.number, worktree.path()),
            slots: slots.clone(),
            sink: self.sink(Step::Agent),
        };
        self.step(Step::Agent, implement.run()).await?;
        let pushed = self.step(Step::Commit, self.committed(linked)).await?;
        let push = Push {
            worktree: worktree.clone(),
            branch: linked.branch().to_owned(),
            credentials,
            slots: slots.clone(),
        };
        self.step(Step::Push, push.run()).await?;
        Ok(pushed)
    }

    /// The tip of the issue's branch once everything in `linked` is
    /// committed. Whether work happened is read from git: an agent
    /// that committed its own work leaves a clean worktree on a branch
    /// that has moved from its [`cut`](LinkedWorktree::cut), and its tip
    /// is the work.
    ///
    /// # Errors
    ///
    /// [`NothingToCommit`] for a clean worktree still at its cut.
    async fn committed(&self, linked: &LinkedWorktree) -> Result<Hash> {
        let slots = &self.build.slots;
        let commit = Commit {
            worktree: linked.worktree().clone(),
            message: format!("Implement #{}", self.number),
            slots: slots.clone(),
        };
        match commit.commit().await {
            Err(error) if error.is::<NothingToCommit>() => {
                let worktree = linked.worktree().clone();
                let tip = slots.run(|| head(worktree)).await?;
                if tip == *linked.cut() {
                    Err(error)
                } else {
                    Ok(tip)
                }
            }
            made => made,
        }
    }

    fn sink(&self, step: Step) -> Sink<Happened> {
        self.build.sink.node(Name::Step(self.number, step))
    }

    /// `work` as `step` of this issue, its lifecycle on the stream.
    async fn step<R>(&self, step: Step, work: impl Future<Output = Result<R>>) -> Result<R> {
        reported(&self.sink(step), work).await
    }
}

/// The commit `worktree` stands on.
fn head(worktree: Repository) -> Result<Hash> {
    git::rev_parse(RevParse::new(worktree, "HEAD".to_owned())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Update;
    use crate::dag::Node as After;
    use crate::git::commands::Branch;
    use crate::git::testing::{guarded, pushed, seeded};
    use crate::github::fake::{self, FakeGitHub};
    use crate::github::{Conclusion, Error, State};
    use crate::plan::github::feature_branch;
    use crate::plan::stream::received;
    use crate::process::Process;
    use crate::temp::Scratch;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::time::{sleep, timeout};

    /// Long enough for anything here that makes progress at all.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// The feature under construction, in these tests.
    const FEATURE: u64 = 7;

    /// The scripted engine: `sh -c` of a script, in the worktree. Each
    /// line it says is its text.
    #[derive(Clone)]
    struct Script {
        cwd: PathBuf,
        script: String,
    }

    impl Agent for Script {
        fn start(self) -> Result<Process> {
            Process::new("sh", vec!["-c".to_owned(), self.script], self.cwd, [], None)
        }

        fn interpret(line: &str) -> Vec<Update> {
            vec![text(line)]
        }
    }

    fn text(text: &str) -> Update {
        Update::Text {
            text: text.to_owned(),
        }
    }

    /// A script that says so and leaves one new file uncommitted.
    fn edits(issue: u64) -> String {
        format!("echo 'implementing {issue}'; echo work > issue-{issue}.txt")
    }

    /// A plan of `issue` alone.
    fn alone(issue: u64) -> Dag<u64> {
        Dag::new(vec![After::new(issue, [])]).unwrap()
    }

    /// #158's shape: 156 after 154 and 155, and 157 after 156.
    fn diamond() -> Dag<u64> {
        Dag::new(vec![
            After::new(154, []),
            After::new(155, []),
            After::new(156, [0, 1]),
            After::new(157, [2]),
        ])
        .unwrap()
    }

    /// A repository as GitHub and as git hold it, neither knowing the
    /// other: a fake GitHub where feature 7 and its `issues` are open,
    /// and a bare origin where the feature branch already stands. Each
    /// issue's branch stands on the fake from the start, since a push
    /// to the origin never reaches it.
    struct Hosted {
        scratch: Scratch,
        origin: Repository,
        fake: FakeGitHub,
    }

    impl Hosted {
        fn new(name: &str, issues: &[u64]) -> Self {
            let scratch = Scratch::new(name);
            let (origin, work) = seeded(&scratch);
            let branch = feature_branch(FEATURE);
            git::branch(Branch::new(work.clone(), branch.clone(), None).unwrap()).unwrap();
            pushed(&work, &branch);
            let fake = FakeGitHub::spawn();
            {
                let mut world = fake.world();
                world.open_issue(FEATURE, "Build the thing");
                for &issue in issues {
                    world.open_issue(issue, &format!("Lay stone {issue}"));
                    world
                        .branches
                        .insert(issue_branch(issue), fake::TIP.to_owned());
                }
            }
            Self {
                scratch,
                origin,
                fake,
            }
        }

        /// A build of the feature in `slots` slots, each issue's agent
        /// running the script `scripts` gives it, with no checks
        /// expected; and everything the build emits.
        fn build(
            &self,
            slots: usize,
            scripts: impl Fn(u64) -> String + Send + Sync + 'static,
        ) -> (Build<Script>, UnboundedReceiver<Event<Name>>) {
            let (sink, receiver) = Sink::channel();
            let build = Build {
                feature: Feature {
                    github: self.fake.github(),
                    repo: FakeGitHub::repo(),
                    number: FEATURE,
                },
                url: Url::Local(self.origin.path().to_owned()),
                worktrees: Worktrees::new(self.scratch.join("root").into()),
                checks: CheckWait {
                    expected: false,
                    every: Duration::from_millis(5),
                    bound: PATIENCE,
                },
                merge: Merge::Squash,
                slots: ProcessSlots::new(NonZeroUsize::new(slots).unwrap()),
                sink,
                agent: Box::new(move |issue, cwd| Script {
                    cwd: cwd.to_owned(),
                    script: scripts(issue),
                }),
            };
            (build, receiver)
        }

        /// The commit `branch` stands on in the origin, once pushed.
        fn tip(&self, branch: &str) -> Option<Hash> {
            git::rev_parse(RevParse::new(self.origin.clone(), branch.to_owned()).unwrap()).ok()
        }

        /// Whether the fake has been asked for the check runs of `sha`.
        fn polled(&self, sha: &Hash) -> bool {
            let read = fake::check_runs(&sha.to_string());
            self.fake.requests().contains(&read)
        }

        fn green(&self, sha: &Hash) {
            let green = vec![fake::check("test", Some(Conclusion::Success))];
            self.fake.world().checks.insert(sha.to_string(), green);
        }

        fn state(&self, issue: u64) -> State {
            self.fake.world().issues[&issue].state
        }

        /// The head of each pull request opened, and whether it is
        /// merged, in the order opened.
        fn pulls(&self) -> Vec<(String, bool)> {
            let world = self.fake.world();
            let pulls = world.pulls.iter();
            pulls
                .map(|pull| (pull.head.name.clone(), pull.merged))
                .collect()
        }
    }

    /// Where `happened` at `name` sits among `events`.
    fn at(events: &[Event<Name>], name: Name, happened: &Happened) -> Option<usize> {
        events
            .iter()
            .position(|event| event.node == name && event.happened == *happened)
    }

    /// Everything that happened at `name`, in order.
    fn happened(events: &[Event<Name>], name: Name) -> Vec<Happened> {
        let there = events.iter().filter(|event| event.node == name);
        there.map(|event| event.happened.clone()).collect()
    }

    /// The words `name` failed in.
    fn failure(events: &[Event<Name>], name: Name) -> String {
        match happened(events, name).last() {
            Some(Happened::Failed(words)) => words.clone(),
            other => panic!("{name:?} failed: {other:?}"),
        }
    }

    #[test]
    fn the_shape_of_158_is_six_nodes_with_every_issue_between_the_branch_and_the_pull_request() {
        let hosted = Hosted::new("feature-shape", &[]);
        let plan = hosted.build(1, edits).0.plan(diamond());

        let names: Vec<_> = plan.nodes().map(|node| node.value().task.name()).collect();
        let issues = [154, 155, 156, 157].map(Name::Issue);
        assert_eq!(names.len(), 6);
        assert_eq!(names[0], Name::Branch);
        assert_eq!(names[1..5], issues);
        assert_eq!(names[5], Name::PullRequest);

        let issue = |name| plan.find(|node| node.task.name() == name).unwrap();
        for name in issues {
            let mut before = issue(name).ancestors();
            assert!(before.any(|node| node.value().task.name() == Name::Branch));
            let mut after = issue(name).descendants();
            assert!(after.any(|node| node.value().task.name() == Name::PullRequest));
        }
        let directly_before = |name| {
            let before = issue(name).predecessors();
            before
                .map(|node| node.value().task.name())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            directly_before(issues[2]),
            [Name::Branch, issues[0], issues[1]]
        );
        assert_eq!(directly_before(issues[3]), [Name::Branch, issues[2]]);
    }

    #[tokio::test]
    async fn a_feature_with_no_open_issue_is_its_branch_and_then_its_pull_request() {
        let hosted = Hosted::new("feature-empty", &[]);
        let (build, _receiver) = hosted.build(1, edits);
        let plan = build.plan(Dag::new(vec![]).unwrap());
        timeout(PATIENCE, plan.execute()).await.unwrap().unwrap();
        assert_eq!(hosted.pulls(), [(feature_branch(FEATURE), false)]);
    }

    #[tokio::test]
    async fn every_issue_is_built_after_the_branch_and_merged_before_the_pull_request_opens() {
        let hosted = Hosted::new("feature-built", &[154, 155, 156, 157]);
        let (build, mut receiver) = hosted.build(2, edits);
        let plan = build.plan(diamond());
        timeout(PATIENCE, plan.execute()).await.unwrap().unwrap();

        let events = received(&mut receiver);
        let started = |name| at(&events, name, &Happened::Started).unwrap();
        let finished = |name| at(&events, name, &Happened::Finished).unwrap();
        for issue in [154, 155, 156, 157] {
            let name = Name::Issue(issue);
            assert!(finished(Name::Branch) < started(name), "{events:?}");
            assert!(finished(name) < started(Name::PullRequest), "{events:?}");
            let said = Happened::Said(text(&format!("implementing {issue}")));
            assert!(at(&events, Name::Step(issue, Step::Agent), &said).is_some());
            let tip = hosted.tip(&issue_branch(issue)).expect("a pushed branch");
            assert!(
                hosted.polled(&tip),
                "the checks read are the pushed commit's"
            );
            assert_eq!(hosted.state(issue), State::Closed);
        }
        let [first, second, third, fourth] = [154, 155, 156, 157].map(Name::Issue);
        assert!(finished(first) < started(third) && finished(second) < started(third));
        assert!(finished(third) < started(fourth));

        let mut pulls = hosted.pulls();
        assert_eq!(pulls.pop(), Some((feature_branch(FEATURE), false)));
        pulls.sort();
        let merged = [154, 155, 156, 157].map(|issue| (issue_branch(issue), true));
        assert_eq!(pulls, merged);
        assert_eq!(hosted.state(FEATURE), State::Open);
        let root = hosted.scratch.0.join("root");
        assert!(
            root.read_dir().unwrap().next().is_none(),
            "no worktree stays"
        );
    }

    #[tokio::test]
    async fn a_failing_issue_holds_back_its_dependents_and_the_pull_request_while_the_rest_finish()
    {
        let hosted = Hosted::new("feature-failing", &[154, 155, 156, 157]);
        let (build, mut receiver) = hosted.build(2, |issue| match issue {
            155 => "exit 3".to_owned(),
            issue => edits(issue),
        });
        let plan = build.plan(diamond());
        let error = timeout(PATIENCE, plan.execute())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "the agent exited with code 3");

        let events = received(&mut receiver);
        assert_eq!(hosted.state(154), State::Closed, "the independent issue");
        assert_eq!(hosted.pulls(), [(issue_branch(154), true)]);
        for unrun in [Name::Issue(156), Name::Issue(157), Name::PullRequest] {
            assert_eq!(happened(&events, unrun), [], "{unrun:?} never ran");
        }
        for open in [155, 156, 157] {
            assert_eq!(hosted.state(open), State::Open);
        }
    }

    #[tokio::test]
    async fn an_agent_that_exits_nonzero_fails_its_issue_with_the_code_in_the_words() {
        let hosted = Hosted::new("feature-exit", &[3]);
        let (build, mut receiver) = hosted.build(1, |_| "echo 'no good' >&2; exit 7".to_owned());
        let error = timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "the agent exited with code 7");

        let events = received(&mut receiver);
        let words = "the agent exited with code 7".to_owned();
        assert_eq!(
            happened(&events, Name::Step(3, Step::Agent)),
            [
                Happened::Started,
                Happened::Stderr("no good".to_owned()),
                Happened::Failed(words.clone()),
            ]
        );
        assert_eq!(
            happened(&events, Name::Issue(3)),
            [Happened::Started, Happened::Failed(words)]
        );
        assert_eq!(hosted.tip("issue-3"), None, "nothing was pushed");
    }

    #[tokio::test]
    async fn a_sequence_stops_at_its_first_failing_step_whose_words_are_the_issues() {
        let hosted = Hosted::new("feature-refused", &[3]);
        hosted.fake.world().merge_refusal = Some("Pull Request is not mergeable".to_owned());
        let (build, mut receiver) = hosted.build(1, edits);
        let error = timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.is::<Error>(), "{error}");

        let events = received(&mut receiver);
        let ended: Vec<_> = events
            .iter()
            .filter_map(|event| match (event.node, &event.happened) {
                (Name::Step(3, step), Happened::Finished | Happened::Failed(_)) => Some(step),
                _ => None,
            })
            .collect();
        let steps = [
            Step::Worktree,
            Step::Agent,
            Step::Commit,
            Step::Push,
            Step::PullRequest,
            Step::Checks,
            Step::Merge,
        ];
        assert_eq!(ended, steps, "and no close");
        let words = failure(&events, Name::Step(3, Step::Merge));
        assert!(words.contains("Pull Request is not mergeable"), "{words}");
        assert_eq!(failure(&events, Name::Issue(3)), words);
        assert_eq!(hosted.state(3), State::Open);
    }

    #[tokio::test]
    async fn with_one_slot_an_agent_runs_while_another_issue_waits_on_its_checks() {
        // 1 and 2 start together and 3 follows 2. The checks of 2 turn
        // green once 1 is seen waiting on its own, which stay pending
        // until the agent of 3 has come and gone in the one slot.
        let hosted = Hosted::new("feature-one-slot", &[1, 2, 3]);
        let (mut build, mut receiver) = hosted.build(1, edits);
        build.checks.expected = true;
        let work = vec![After::new(1, []), After::new(2, []), After::new(3, [1])];
        let run = tokio::spawn(build.plan(Dag::new(work).unwrap()).execute());

        let agent = Name::Step(3, Step::Agent);
        let mut events = Vec::new();
        let built = timeout(PATIENCE, async {
            while !run.is_finished() {
                sleep(Duration::from_millis(5)).await;
                events.extend(received(&mut receiver));
                let tips = [1, 2, 3].map(|issue| hosted.tip(&issue_branch(issue)));
                let [Some(first), rest @ ..] = tips else {
                    continue;
                };
                if hosted.polled(&first) {
                    rest.iter().flatten().for_each(|tip| hosted.green(tip));
                }
                if at(&events, agent, &Happened::Finished).is_some() {
                    hosted.green(&first);
                }
            }
            run.await
        });
        built.await.unwrap().unwrap().unwrap();

        events.extend(received(&mut receiver));
        let waiting = Name::Step(1, Step::Checks);
        let order = [
            at(&events, waiting, &Happened::Started),
            at(&events, agent, &Happened::Started),
            at(&events, agent, &Happened::Finished),
            at(&events, waiting, &Happened::Finished),
        ];
        assert!(order[0].is_some() && order.is_sorted(), "{order:?}");
    }

    #[tokio::test]
    async fn a_remote_that_asks_is_answered_with_the_token_the_github_calls_are_made_with() {
        let hosted = Hosted::new("feature-guarded", &[3]);
        let (mut build, mut receiver) = hosted.build(1, edits);
        let (url, seen) = guarded();
        build.url = url.parse().unwrap();
        timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap_err();

        // Base64 of `x-access-token:ghp_fake`, the fake client's token.
        let offered = "Basic eC1hY2Nlc3MtdG9rZW46Z2hwX2Zha2U=";
        assert_eq!(seen.recv_timeout(PATIENCE).as_deref(), Ok(offered));
        let events = received(&mut receiver);
        let words = failure(&events, Name::Step(3, Step::Worktree));
        assert!(words.contains("Authentication failed"), "{words}");
    }

    #[tokio::test]
    async fn work_an_agent_committed_itself_is_pushed_as_it_stands() {
        let hosted = Hosted::new("feature-own-commit", &[3]);
        let (build, _receiver) = hosted.build(1, |issue| {
            let edits = edits(issue);
            format!("{edits}; git add . && git commit -qm 'my own work'")
        });
        let plan = build.plan(alone(3));
        timeout(PATIENCE, plan.execute()).await.unwrap().unwrap();

        let tip = hosted.tip("issue-3").expect("the agent's commit, pushed");
        assert!(hosted.polled(&tip), "and its checks were the ones read");
        assert_eq!(hosted.state(3), State::Closed);
    }

    #[tokio::test]
    async fn an_agent_that_leaves_the_branch_as_cut_fails_with_nothing_to_commit() {
        let hosted = Hosted::new("feature-no-work", &[3]);
        let (build, mut receiver) = hosted.build(1, |_| "echo 'all done!'".to_owned());
        let error = timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.is::<NothingToCommit>(), "{error}");

        let events = received(&mut receiver);
        let words = failure(&events, Name::Step(3, Step::Commit));
        assert_eq!(words, "the worktree holds nothing to commit");
        assert_eq!(hosted.tip("issue-3"), None, "nothing was pushed");
        assert_eq!(hosted.pulls(), []);
    }

    #[test]
    fn a_name_crosses_as_json_and_comes_back_the_same() {
        let names = [
            (Name::Branch, r#""Branch""#),
            (Name::Issue(203), r#"{"Issue":203}"#),
            (Name::Step(203, Step::Checks), r#"{"Step":[203,"Checks"]}"#),
            (Name::PullRequest, r#""PullRequest""#),
        ];
        for (name, line) in names {
            assert_eq!(serde_json::to_string(&name).unwrap(), line);
            assert_eq!(serde_json::from_str::<Name>(line).unwrap(), name);
        }
    }
}
