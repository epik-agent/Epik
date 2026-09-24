//! A feature build as one plan: the feature's branch, then its issues
//! in the order the tracker gives them, then its pull request.
//!
//! A [`Build`] is everything a feature is built with, and
//! [`Build::plan`] lays the three kinds of [`Node`] over the tracker's
//! [`Dag`] of issue numbers. Every node holds the one build, so every
//! agent any of them launches goes through the one set of slots, and
//! everything any of them says goes into the one sink, under its
//! [`Name`].
//!
//! An [`Issue`] is a [`TaskMachine`] whose states are [`Step`]s, each
//! acted in and each with its lifecycle on the stream under its own
//! [`Name::Step`]. The agent's state holds a slot while it runs; the
//! git states and the states that talk to GitHub hold none, so an
//! issue waiting on its checks — or committing what its agent left —
//! leaves its slot to another issue's agent.
//!
//! The machine has one back-edge: checks that went red send it to a
//! fixing [`Agent`](Step::Agent), which resumes the session the first
//! run opened, and round again to [`Checks`](Step::Checks) through the
//! pull request already open. [`RUNS`] bounds the times it may enter
//! the agent, the first run among them. The worktree the issue is
//! built in lives in the state from the cut until the machine ends, so
//! there is somewhere to fix in.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use super::commit::{Commit, Committed};
use super::github::{CheckWait, CreateBranch, Feature, Merged, OpenPull, Verdict};
use super::machine::{Machine, Next, TaskMachine};
use super::push::Push;
use super::stream::{Event, Happened, Reported, Sink, reported};
use super::task::Task;
use super::{AgentSlots, Implement, Plan, briefly};
use crate::agent::Agent;
use crate::dag::Dag;
use crate::git::askpass::Credentials;
use crate::git::commands::{Diff, Hash};
use crate::git::url::Url;
use crate::git::{self};
use crate::github::{Error, Merge, Required};
pub use crate::happening::{Name, Step};
use crate::worktrees::{LinkedWorktree, Worktrees};

/// How many times one issue's machine may run an agent, its first run
/// among them. An agent run is what costs money, so it is what a budget
/// bounds, and the issue's first attempt and every fix of it draw on the
/// one count. It stands beside the host's `SLOTS` as the other bound on
/// what a build spends: slots say how many agents run at once, and this
/// says how many one issue is worth.
pub const RUNS: usize = 3;

/// The branch issue `number` is implemented on.
#[must_use]
pub fn issue_branch(number: u64) -> String {
    format!("issue-{number}")
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
    /// What the rules on the feature branch require to pass before it
    /// takes a merge, and how the wait for those is paced and bounded.
    /// An empty [`Policy::checks`](crate::plan::policy::Policy::checks)
    /// is a repository requiring none, whose issues merge unchecked.
    pub required: Vec<Required>,
    pub checks: CheckWait,
    /// How an issue's pull request merges into the feature branch.
    pub merge: Merge,
    /// The slots the build's agents run in. Held by the host and
    /// cloned to every build it starts, so builds at once run no more
    /// agents between them than one does.
    pub slots: AgentSlots,
    pub sink: Sink<Event<Name>>,
    pub agent: Agents<A>,
}

/// The agent for one run on the issue numbered, in the worktree at the
/// path, briefed as the [`Errand`] says: made as each run starts, which
/// is when its working directory and its errand are both known.
pub type Agents<A> = Box<dyn Fn(u64, &Path, &Errand) -> A + Send + Sync>;

/// What one agent run is for, and so what the agent is told and
/// whether it comes back to a session. The build's [`Agents`] turns one
/// of these into the agent that does it, because whoever supplies the
/// agents holds the issue's text and the words of every brief.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Errand {
    /// The issue implemented from its text, in a session of its own.
    Implement,
    /// The red checks fixed by the agent that wrote the branch, coming
    /// back to `session`, which holds what it built and why.
    Resume {
        session: String,
        /// The names of the required checks that concluded red.
        checks: String,
    },
    /// The red checks fixed from the record instead: the branch's
    /// `diff` against the feature branch stands in for the session.
    Fresh { diff: String, checks: String },
}

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
            node(Node::Issue(Machine(Issue { number, build })))
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
    /// One issue, implemented and merged into the feature branch: the
    /// machine whose states are its steps, run as a task.
    Issue(Machine<Issue<A>>),
    /// The plan's sink: the feature's pull request, left open.
    PullRequest(OpenPull),
}

impl<A> Node<A> {
    /// What the node's events are filed under.
    #[must_use]
    pub const fn name(&self) -> Name {
        match self {
            Self::Branch(_) => Name::Branch,
            Self::Issue(machine) => Name::Issue(machine.0.number),
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

/// One issue of a feature as a machine: from its worktree, through the
/// agent and the git and the GitHub of it, to its closing — and round
/// again, from red checks to a fixing agent, while the budget lasts.
pub struct Issue<A> {
    number: u64,
    build: Arc<Build<A>>,
}

/// What the issue's commit step came to: whether an agent's run left
/// the branch anywhere, read from git.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Worked {
    /// The commit the branch now stands on, whether the agent made it
    /// or Epik did.
    At(Hash),
    /// A clean worktree on a branch still at its
    /// [cut](LinkedWorktree::cut): the agent left the issue as it
    /// found it.
    Nothing,
}

/// What an issue's machine carries from the cut onwards: the worktree
/// it is built in, who its git answers as, and what the states behind
/// it made.
///
/// The worktree is why the state is moved through
/// [`next`](TaskMachine::next) rather than cloned: it is `Drop`-owned,
/// and holding it here is what keeps it standing until the machine
/// ends, however the machine ends.
pub struct Held {
    worktree: LinkedWorktree,
    credentials: Credentials,
    /// The times the machine has entered [`Step::Agent`], its first run
    /// among them, against the [`RUNS`] it may.
    runs: usize,
    /// The session the first agent run opened, for a fix to come back
    /// to.
    session: Option<String>,
    /// The names of the required checks the next agent run is to fix:
    /// what a red verdict left behind it. The first run, which has no
    /// verdict behind it, implements the issue instead.
    fix: Option<String>,
    /// The commit the branch was last pushed at, which is the one the
    /// pull request, the checks and the merge each name.
    pushed: Option<Hash>,
    /// The pull request open on the issue's branch, once one is. A
    /// second time round, this is why [`Step::PullRequest`] is skipped.
    pull: Option<u64>,
}

/// Where an issue's machine is, and what it carries there.
pub enum State {
    /// Before the cut, holding nothing: the machine's first state.
    Cutting,
    /// In the worktree, with `step` to act in. What it carries there is
    /// boxed, so a state before the cut is the size of a step rather
    /// than of everything a later one holds.
    In { step: Step, held: Box<Held> },
}

/// What acting in one of an issue's states produced, as the value
/// [`next`](TaskMachine::next) reads.
pub enum Acted {
    /// The worktree cut, and the credentials its git answers as.
    Cut(LinkedWorktree, Credentials),
    /// An agent's run that exited well, with the session it opened when
    /// it opened one.
    Ran(Option<String>),
    /// What the worktree held, committed.
    Committed(Worked),
    /// The branch pushed, at the commit the state already carries.
    Pushed,
    /// The pull request on the branch, by number.
    Opened(u64),
    /// What the wait for the required checks came to.
    Checked(Verdict),
    /// What asking GitHub for the merge came to.
    Merged(Merged),
    /// The issue closed.
    Closed,
}

/// How `count` agent runs are counted in English, so the failure of a
/// spent budget reads as one whatever [`RUNS`] is.
fn runs(count: usize) -> &'static str {
    if count == 1 { "run" } else { "runs" }
}

impl<A: Agent + Clone + Send + Sync + 'static> TaskMachine for Issue<A> {
    type State = State;
    type Outcome = Acted;

    fn start(&self) -> State {
        State::Cutting
    }

    /// The state's own work, reported under its [`Name::Step`]. Every
    /// entry into a step is reported, so a step the machine comes back
    /// to is started and ended again under the name it had before.
    ///
    /// # Errors
    ///
    /// Whatever the step's work fails with, which is the machine's
    /// failure. An outcome the machine branches on is none of those: a
    /// red verdict, a refused merge and a worktree holding nothing each
    /// come back as values.
    async fn act(&self, state: &State) -> Result<Acted> {
        match state {
            State::Cutting => self.step(Step::Worktree, self.cut()).await,
            State::In { step, held } => self.step(*step, self.acting(*step, held)).await,
        }
    }

    /// The transition table: where the machine goes from `state` given
    /// what acting there produced. No I/O, no clock, no git — every
    /// branch reads a value — and the state is moved, so what it
    /// carries either goes on or goes.
    ///
    /// The one back-edge is a red verdict with runs left, which returns
    /// to [`Step::Agent`] carrying the red names for its brief and comes
    /// round to [`Step::Checks`] through the pull request the state
    /// already holds.
    fn next(&self, state: State, outcome: Acted) -> Next<State> {
        let State::In { step, mut held } = state else {
            let Acted::Cut(worktree, credentials) = outcome else {
                return Next::Fail(astray(Step::Worktree));
            };
            return Next::Go(State::In {
                step: Step::Agent,
                held: Box::new(Held {
                    worktree,
                    credentials,
                    runs: 0,
                    session: None,
                    fix: None,
                    pushed: None,
                    pull: None,
                }),
            });
        };
        let onward = match (step, outcome) {
            (Step::Agent, Acted::Ran(session)) => {
                held.runs += 1;
                // The session worth resuming is the first run's: that
                // is the one that holds how the branch came to be.
                held.session = held.session.or(session);
                held.fix = None;
                Step::Commit
            }
            (Step::Commit, Acted::Committed(Worked::Nothing)) => {
                return Next::Fail(anyhow!("the worktree holds nothing to commit"));
            }
            (Step::Commit, Acted::Committed(Worked::At(hash))) => {
                held.pushed = Some(hash);
                Step::Push
            }
            // The pull request already open follows the branch, so a
            // second push onto it needs only its checks read again.
            (Step::Push, Acted::Pushed) => match held.pull {
                Some(_) => Step::Checks,
                None => Step::PullRequest,
            },
            (Step::PullRequest, Acted::Opened(pull)) => {
                held.pull = Some(pull);
                Step::Checks
            }
            (Step::Checks, Acted::Checked(Verdict::Red(names))) if held.runs < RUNS => {
                held.fix = Some(names);
                Step::Agent
            }
            (Step::Checks, Acted::Checked(Verdict::Red(names))) => {
                return Next::Fail(anyhow!(
                    "checks failed on {}: {names}, after {} agent {}",
                    commit(&held),
                    held.runs,
                    runs(held.runs)
                ));
            }
            (Step::Checks, Acted::Checked(Verdict::Green | Verdict::Unclaimed)) => Step::Merge,
            (Step::Merge, Acted::Merged(Merged::Refused(words))) => {
                return Next::Fail(
                    Error::Refused {
                        status: 405,
                        message: words,
                    }
                    .into(),
                );
            }
            (Step::Merge, Acted::Merged(Merged::Done)) => Step::Close,
            (Step::Close, Acted::Closed) => return Next::Done,
            (step, _) => return Next::Fail(astray(step)),
        };
        Next::Go(State::In { step: onward, held })
    }
}

/// The commit the issue's branch stands pushed at, which is what the
/// pull request, the checks and the merge each name. Every state that
/// asks is reached through a push, so the commit is there; a state
/// reached another way says where it stands instead of naming a commit
/// as its own.
fn commit(held: &Held) -> String {
    held.pushed
        .as_ref()
        .map_or_else(|| "a branch yet to be pushed".to_owned(), Hash::to_string)
}

/// The failure of a step handed the outcome of another step: the states
/// and the outcomes are two vocabularies, and this is what a pairing of
/// them the table has no edge for says.
fn astray(step: Step) -> anyhow::Error {
    anyhow!("{step:?} came to an outcome of another step's")
}

impl<A: Agent + Clone + Send + Sync + 'static> Issue<A> {
    /// The worktree for the issue's branch, cut at the feature branch's
    /// tip, with the credentials its git and its pushes answer with.
    /// Git, and so unslotted: short, and free to run while every slot
    /// is out.
    async fn cut(&self) -> Result<Acted> {
        let Build {
            feature,
            url,
            worktrees,
            ..
        } = &*self.build;
        let (worktrees, url) = (worktrees.clone(), url.clone());
        let (branch, at) = (
            issue_branch(self.number),
            format!("origin/{}", feature.branch()),
        );
        let github = feature.github.clone();
        briefly(move || {
            let credentials = github.credentials()?;
            let linked = worktrees.add(&url, &branch, &at, &credentials)?;
            Ok(Acted::Cut(linked, credentials))
        })
        .await
    }

    /// What acting in `step` comes to, given what the states behind it
    /// left in `held`.
    async fn acting(&self, step: Step, held: &Held) -> Result<Acted> {
        let Build {
            feature,
            required,
            checks,
            merge,
            ..
        } = &*self.build;
        match step {
            // The cut is the machine's one state before the worktree,
            // and is `cut`'s to act in.
            Step::Worktree => Err(astray(step)),
            Step::Agent => self.ran(held).await,
            Step::Commit => self.committed(held).await.map(Acted::Committed),
            Step::Push => self.pushed(held).await,
            Step::PullRequest => {
                let branch = issue_branch(self.number);
                let pull = feature.open_issue_pull(self.number, &branch).await?;
                Ok(Acted::Opened(pull.number))
            }
            Step::Checks => {
                let judged = commit(held);
                let said = self.sink(step);
                let verdict = feature
                    .await_checks(&judged, required, *checks, &said)
                    .await?;
                Ok(Acted::Checked(verdict))
            }
            Step::Merge => {
                let pull = held.pull.ok_or_else(|| astray(step))?;
                let merged = feature.merge(pull, &commit(held), *merge).await?;
                Ok(Acted::Merged(merged))
            }
            Step::Close => feature.close(self.number).await.map(|()| Acted::Closed),
        }
    }

    /// One agent's run in the worktree: the errand `held` calls for,
    /// run to its exit in a slot, yielding the session it opened.
    ///
    /// A fix resumes the session the first run opened. A resumed run
    /// that comes back a failure — the session gone, the CLI refusing
    /// it — is followed by a fresh one, briefed with the branch's diff
    /// against the feature branch where the session would have stood.
    /// The two attempts are the one run of the budget, because what the
    /// fix cost is one agent's work on it.
    async fn ran(&self, held: &Held) -> Result<Acted> {
        let Some(checks) = held.fix.clone() else {
            return self.sent(held, &Errand::Implement).await;
        };
        if let Some(session) = held.session.clone() {
            let checks = checks.clone();
            let resumed = Errand::Resume { session, checks };
            match self.sent(held, &resumed).await {
                Err(refused) => self.sink(Step::Agent).emit(Happened::Stderr(format!(
                    "the session could not be resumed, so the fix runs fresh: {refused:#}"
                ))),
                ran => return ran,
            }
        }
        let diff = self.diff(held).await?;
        self.sent(held, &Errand::Fresh { diff, checks }).await
    }

    /// The agent the build makes for `errand`, run to its exit in one of
    /// the slots, in the issue's worktree, with everything it says going
    /// out under [`Step::Agent`].
    async fn sent(&self, held: &Held, errand: &Errand) -> Result<Acted> {
        let Build { slots, agent, .. } = &*self.build;
        let worktree = held.worktree.worktree().path();
        let implement = Implement {
            agent: agent(self.number, worktree, errand),
            slots: slots.clone(),
            sink: self.sink(Step::Agent),
        };
        implement.implemented().await.map(Acted::Ran)
    }

    /// The branch's diff against the feature branch: what the branch so
    /// far amounts to, which a fresh fix reads where a resumed one would
    /// have remembered it.
    async fn diff(&self, held: &Held) -> Result<String> {
        let against = format!("origin/{}..HEAD", self.build.feature.branch());
        let diff = Diff::new(held.worktree.worktree().clone(), Some(against), None)?;
        briefly(move || git::diff(diff)).await
    }

    /// Everything the worktree holds, committed, and the tip of the
    /// issue's branch once it is. Whether work happened is read from
    /// git: an agent that committed its own work leaves a clean
    /// worktree on a branch that has moved from its
    /// [`cut`](LinkedWorktree::cut), and its tip is the work.
    ///
    /// The tip read is the branch's own, which is the commit the push,
    /// the pull request and the checks each go on to name. An agent
    /// that left the worktree somewhere else is a fact to state, so
    /// [`Commit`] refuses rather than committing off the branch.
    ///
    /// # Errors
    ///
    /// A worktree off the issue's branch.
    async fn committed(&self, held: &Held) -> Result<Worked> {
        let linked = &held.worktree;
        let commit = Commit {
            worktree: linked.worktree().clone(),
            branch: linked.branch().to_owned(),
            message: format!("Implement #{}", self.number),
        };
        match commit.commit().await? {
            Committed::Made(hash) => Ok(Worked::At(hash)),
            Committed::Nothing => {
                let tip = commit.tip().await?;
                if tip == *linked.cut() {
                    Ok(Worked::Nothing)
                } else {
                    Ok(Worked::At(tip))
                }
            }
        }
    }

    /// The issue's branch pushed, at the commit the commit step made.
    /// The commit crosses the stream within this step, before its end,
    /// so nothing arrives under a step that has already finished: it is
    /// a fact only once the push has put it on the remote, and it is
    /// the commit every state after this one names.
    async fn pushed(&self, held: &Held) -> Result<Acted> {
        let push = Push {
            worktree: held.worktree.worktree().clone(),
            branch: held.worktree.branch().to_owned(),
            credentials: held.credentials.clone(),
        };
        push.run().await?;
        self.sink(Step::Push).emit(Happened::Pushed(commit(held)));
        Ok(Acted::Pushed)
    }

    fn sink(&self, step: Step) -> Sink<Happened> {
        self.build.sink.node(Name::Step(self.number, step))
    }

    /// `work` as `step` of this issue, its lifecycle on the stream.
    async fn step<R>(&self, step: Step, work: impl Future<Output = Result<R>>) -> Result<R> {
        reported(&self.sink(step), work).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Update;
    use crate::dag::Node as After;
    use crate::git::Repository;
    use crate::git::commands::{Branch, RevParse};
    use crate::git::testing::{guarded, published, pushed};
    use crate::git::{self};
    use crate::github::fake::{self, FakeGitHub};
    use crate::github::{Conclusion, Error, State};
    use crate::plan::github::feature_branch;
    use crate::plan::stream::received;
    use crate::process::Process;
    use crate::temp::Scratch;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::time::{sleep, timeout};

    /// Long enough for anything here that makes progress at all.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// The feature under construction, in these tests.
    const FEATURE: u64 = 7;

    /// The rule requiring a check named `context`, from any app.
    fn requires(context: &str) -> Required {
        Required {
            context: context.to_owned(),
            app: None,
        }
    }

    /// The scripted engine: `sh -c` of a script, in the worktree. A
    /// line beginning [`OPENS`] is the session it opened, and every
    /// other line it says is its text.
    #[derive(Clone)]
    struct Script {
        cwd: PathBuf,
        script: String,
    }

    /// How a script says which session it is speaking in, which a real
    /// engine says in its own protocol.
    const OPENS: &str = "session ";

    impl Agent for Script {
        fn start(self) -> Result<Process> {
            Process::new("sh", vec!["-c".to_owned(), self.script], self.cwd, [], None)
        }

        fn interpret(line: &str) -> Vec<Update> {
            match line.strip_prefix(OPENS) {
                Some(id) => vec![Update::Session {
                    id: id.to_owned(),
                    model: "scripted".to_owned(),
                }],
                None => vec![text(line)],
            }
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

    /// A script that says so and leaves one more line in the issue's
    /// file: what a fixing run does, as distinguishable from the first
    /// run's work as a commit needs to be.
    fn fixes(issue: u64, run: usize) -> String {
        format!("echo 'fixing {issue}'; echo 'fix {run}' >> issue-{issue}.txt")
    }

    /// Whatever the errand asks for: the first run implements, and every
    /// fix leaves another line. The run is counted so each fix commits
    /// something of its own.
    fn implements_then_fixes() -> impl Fn(u64, &Errand) -> String + Send + Sync + 'static {
        let runs = Arc::new(Mutex::new(0));
        move |issue, errand| {
            let mut runs = runs.lock().unwrap();
            *runs += 1;
            match errand {
                Errand::Implement => edits(issue),
                Errand::Resume { .. } | Errand::Fresh { .. } => fixes(issue, *runs),
            }
        }
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

    /// A repository as GitHub and as git hold it: a fake GitHub where
    /// feature 7 and its `issues` are open, and a bare origin where the
    /// feature branch already stands. The two are one repository, so
    /// the fake reads an issue's branch out of the origin the build
    /// pushes to, the way a push reaches GitHub.
    struct Hosted {
        scratch: Scratch,
        origin: Repository,
        fake: FakeGitHub,
        /// Every errand the build handed an agent, in the order handed.
        errands: Arc<Mutex<Vec<Errand>>>,
    }

    impl Hosted {
        fn new(name: &str, issues: &[u64]) -> Self {
            let scratch = Scratch::new(name);
            let (origin, work) = published(&scratch);
            let branch = feature_branch(FEATURE);
            git::branch(Branch::new(work.clone(), branch.clone(), None).unwrap()).unwrap();
            pushed(&work, &branch);
            let fake = FakeGitHub::spawn();
            {
                let mut world = fake.world();
                world.open_issue(FEATURE, "Build the thing");
                for &issue in issues {
                    world.open_issue(issue, &format!("Lay stone {issue}"));
                }
                let pushed_to = origin.clone();
                world.pushes_to = Some(Box::new(move |branch| {
                    let asked = RevParse::new(pushed_to.clone(), branch.to_owned()).ok()?;
                    git::rev_parse(asked).ok().map(|tip| tip.to_string())
                }));
            }
            Self {
                scratch,
                origin,
                fake,
                errands: Arc::default(),
            }
        }

        /// Every errand an agent of this build was given, in order.
        fn errands(&self) -> Vec<Errand> {
            self.errands.lock().unwrap().clone()
        }

        /// A build of the feature in `slots` slots, whose every agent
        /// run is the issue's own script whatever the errand.
        fn build(
            &self,
            slots: usize,
            scripts: impl Fn(u64) -> String + Send + Sync + 'static,
        ) -> (Build<Script>, UnboundedReceiver<Event<Name>>) {
            self.briefed(slots, move |issue, _| scripts(issue))
        }

        /// A build of the feature in `slots` slots, each agent run
        /// running the script `scripts` gives its errand, with no check
        /// required; and everything the build emits. Every errand is
        /// recorded as it is handed over, so a test reads what each run
        /// was told and whether it came back to a session.
        fn briefed(
            &self,
            slots: usize,
            scripts: impl Fn(u64, &Errand) -> String + Send + Sync + 'static,
        ) -> (Build<Script>, UnboundedReceiver<Event<Name>>) {
            let (sink, receiver) = Sink::channel();
            let errands = Arc::clone(&self.errands);
            let build = Build {
                feature: Feature {
                    github: self.fake.github(),
                    repo: FakeGitHub::repo(),
                    number: FEATURE,
                },
                url: Url::Local(self.origin.path().to_owned()),
                worktrees: Worktrees::new(self.scratch.join("root").into()),
                required: Vec::new(),
                checks: CheckWait {
                    every: Duration::from_millis(5),
                    bound: PATIENCE,
                },
                merge: Merge::Squash,
                slots: AgentSlots::new(NonZeroUsize::new(slots).unwrap()),
                sink,
                agent: Box::new(move |issue, cwd, errand| {
                    errands.lock().unwrap().push(errand.clone());
                    Script {
                        cwd: cwd.to_owned(),
                        script: scripts(issue, errand),
                    }
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

        fn red(&self, sha: &Hash) {
            let red = vec![
                fake::check("build", Some(Conclusion::Success)),
                fake::check("test", Some(Conclusion::Failure)),
            ];
            self.fake.world().checks.insert(sha.to_string(), red);
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
    fn recorded(events: &[Event<Name>], name: Name) -> Vec<Happened> {
        let there = events.iter().filter(|event| event.node == name);
        there.map(|event| event.happened.clone()).collect()
    }

    /// The words `name` failed in.
    fn failure(events: &[Event<Name>], name: Name) -> String {
        match recorded(events, name).last() {
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
            assert!(
                before.any(|node| node.value().task.name() == Name::Branch),
                "{name:?} comes after the branch"
            );
            let mut after = issue(name).descendants();
            assert!(
                after.any(|node| node.value().task.name() == Name::PullRequest),
                "{name:?} comes before the pull request"
            );
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
            assert!(
                at(&events, Name::Step(issue, Step::Agent), &said).is_some(),
                "{events:?}"
            );
            hosted.tip(&issue_branch(issue)).expect("a pushed branch");
            assert_eq!(hosted.state(issue), State::Closed);
        }
        let [first, second, third, fourth] = [154, 155, 156, 157].map(Name::Issue);
        assert!(
            finished(first) < started(third) && finished(second) < started(third),
            "157 waits on both the issues before it: {events:?}"
        );
        assert!(
            finished(third) < started(fourth),
            "and 158 waits on 157: {events:?}"
        );

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
            assert_eq!(recorded(&events, unrun), [], "{unrun:?} never ran");
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
            recorded(&events, Name::Step(3, Step::Agent)),
            [
                Happened::Started,
                Happened::Stderr("no good".to_owned()),
                Happened::Failed(words.clone()),
            ]
        );
        assert_eq!(
            recorded(&events, Name::Issue(3)),
            [Happened::Started, Happened::Failed(words)]
        );
        assert_eq!(hosted.tip("issue-3"), None, "nothing was pushed");
    }

    /// A merge GitHub refuses as not mergeable is a value the merge
    /// step read and finished on, and the issue's own failure: the
    /// machine has no edge from there, so `Name::Issue(3)` is where the
    /// refusal is reported.
    #[tokio::test]
    async fn a_merge_refused_as_not_mergeable_fails_the_issue_in_githubs_words() {
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
        let words = failure(&events, Name::Issue(3));
        assert!(words.contains("Pull Request is not mergeable"), "{words}");
        assert_eq!(words, format!("{error:#}"));
        assert_eq!(hosted.state(3), State::Open);
    }

    /// A build whose policy requires nothing, on a repository with no
    /// CI: nothing is polled, nothing is waited out, and the bound is
    /// long enough that a wait would show as a hang rather than a pass.
    #[tokio::test]
    async fn a_policy_requiring_no_check_merges_the_issue_claiming_no_verdict() {
        let hosted = Hosted::new("feature-unclaimed", &[3]);
        let (build, mut receiver) = hosted.build(1, edits);
        timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap();

        let events = received(&mut receiver);
        let tip = hosted.tip("issue-3").expect("a pushed branch");
        assert!(!hosted.polled(&tip), "there was nothing to wait for");
        assert_eq!(
            recorded(&events, Name::Step(3, Step::Checks)),
            [
                Happened::Started,
                Happened::Checked(Verdict::Unclaimed),
                Happened::Finished,
            ]
        );
        let merged = (issue_branch(3), true);
        assert!(hosted.pulls().contains(&merged), "{:?}", hosted.pulls());
        assert_eq!(hosted.state(3), State::Closed);
    }

    /// A build whose policy requires `test`, with a `coverage` run that
    /// never concludes: the verdict is the required check's, and the
    /// merge does not wait on the other.
    #[tokio::test]
    async fn a_green_required_check_merges_the_issue_though_an_unrequired_run_still_goes() {
        let hosted = Hosted::new("feature-green", &[3]);
        let (mut build, mut receiver) = hosted.build(1, edits);
        build.required = vec![requires("test")];
        let run = tokio::spawn(build.plan(alone(3)).execute());
        let branch = issue_branch(3);
        timeout(PATIENCE, async {
            while !run.is_finished() {
                sleep(Duration::from_millis(5)).await;
                if let Some(tip) = hosted.tip(&branch) {
                    let runs = vec![
                        fake::check("test", Some(Conclusion::Success)),
                        fake::check("coverage", None),
                    ];
                    hosted.fake.world().checks.insert(tip.to_string(), runs);
                }
            }
            run.await
        })
        .await
        .unwrap()
        .unwrap()
        .unwrap();

        let events = received(&mut receiver);
        assert_eq!(
            recorded(&events, Name::Step(3, Step::Checks)),
            [
                Happened::Started,
                Happened::Ran("coverage (running)".to_owned()),
                Happened::Checked(Verdict::Green),
                Happened::Finished,
            ]
        );
        assert!(
            hosted.pulls().contains(&(branch, true)),
            "{:?}",
            hosted.pulls()
        );
        assert_eq!(hosted.state(3), State::Closed);
    }

    /// Checks red after every run: the machine spends its whole budget
    /// on the fixing agent, and the failure names both the red checks
    /// and what the issue cost.
    #[tokio::test]
    async fn checks_red_after_every_run_fail_the_issue_once_the_budget_is_spent() {
        let hosted = Hosted::new("feature-red", &[3]);
        let (mut build, mut receiver) = hosted.briefed(1, implements_then_fixes());
        build.required = vec![requires("test")];
        let run = tokio::spawn(build.plan(alone(3)).execute());
        let branch = issue_branch(3);
        let error = timeout(PATIENCE, async {
            while !run.is_finished() {
                sleep(Duration::from_millis(5)).await;
                if let Some(tip) = hosted.tip(&branch) {
                    hosted.red(&tip);
                }
            }
            run.await
        })
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
        let pushed = hosted.tip(&branch).expect("the commit the checks judged");
        assert_eq!(
            error.to_string(),
            format!("checks failed on {pushed}: test, after {RUNS} agent runs")
        );

        let events = received(&mut receiver);
        assert_eq!(failure(&events, Name::Issue(3)), error.to_string());
        let started = |step| {
            let name = Name::Step(3, step);
            recorded(&events, name)
                .iter()
                .filter(|happened| **happened == Happened::Started)
                .count()
        };
        assert_eq!(started(Step::Agent), RUNS, "the budget, and no more");
        assert_eq!(started(Step::Checks), RUNS, "read after every one");
        assert_eq!(started(Step::PullRequest), 1, "through the one pull");
        assert_eq!(hosted.state(3), State::Open, "and no merge, and no close");
        assert!(
            hosted
                .scratch
                .0
                .join("root")
                .read_dir()
                .unwrap()
                .next()
                .is_none(),
            "and the worktree is gone"
        );
    }

    /// The session the scripted first run opens, which its fix comes
    /// back to.
    const SESSION: &str = "s-3-first";

    /// The build the loop is exercised with: the first run opens
    /// [`SESSION`] and leaves work, and every fix leaves one more line.
    /// What each run was told is in the errands the build recorded.
    fn loops() -> impl Fn(u64, &Errand) -> String + Send + Sync + 'static {
        let runs = Arc::new(Mutex::new(0));
        move |issue, errand| {
            let mut runs = runs.lock().unwrap();
            *runs += 1;
            match errand {
                Errand::Implement => {
                    format!("echo '{OPENS}{SESSION}'; {}", edits(issue))
                }
                Errand::Resume { .. } | Errand::Fresh { .. } => fixes(issue, *runs),
            }
        }
    }

    /// The commits `branch` has stood on in the origin, each read as the
    /// build pushed it: the checks are turned by `turn`, which is given
    /// the tip each time it is seen to have moved.
    async fn watched(
        hosted: &Hosted,
        run: &tokio::task::JoinHandle<Result<()>>,
        branch: &str,
        turn: impl Fn(usize, &Hash),
    ) -> Vec<Hash> {
        let mut pushed: Vec<Hash> = Vec::new();
        while !run.is_finished() {
            sleep(Duration::from_millis(5)).await;
            let Some(tip) = hosted.tip(branch) else {
                continue;
            };
            if pushed.last() != Some(&tip) {
                pushed.push(tip.clone());
            }
            turn(pushed.len(), &tip);
        }
        pushed
    }

    /// The acceptance of the back-edge: checks that go red and then
    /// green merge the issue after two agent runs, the second resuming
    /// the session the first opened, through the one pull request.
    #[tokio::test]
    async fn checks_red_then_green_merge_the_issue_after_a_fix_that_resumed_the_session() {
        let hosted = Hosted::new("feature-fixed", &[3]);
        let (mut build, mut receiver) = hosted.briefed(1, loops());
        build.required = vec![requires("test")];
        let run = tokio::spawn(build.plan(alone(3)).execute());
        let branch = issue_branch(3);
        // The first commit pushed goes red, and the fix's goes green.
        let pushed = timeout(
            PATIENCE,
            watched(&hosted, &run, &branch, |seen, tip| match seen {
                1 => hosted.red(tip),
                _ => hosted.green(tip),
            }),
        )
        .await
        .unwrap();
        timeout(PATIENCE, run).await.unwrap().unwrap().unwrap();

        assert_eq!(pushed.len(), 2, "the branch was pushed twice");
        assert_eq!(
            hosted.errands(),
            [
                Errand::Implement,
                Errand::Resume {
                    session: SESSION.to_owned(),
                    checks: "test".to_owned(),
                },
            ],
            "the fix came back to the first run's session"
        );

        let events = received(&mut receiver);
        let entered = |step| {
            let name = Name::Step(3, step);
            recorded(&events, name)
                .iter()
                .filter(|happened| **happened == Happened::Started)
                .count()
        };
        assert_eq!(entered(Step::Agent), 2, "two agent runs");
        assert_eq!(entered(Step::Commit), 2);
        assert_eq!(entered(Step::Push), 2);
        assert_eq!(entered(Step::Checks), 2);
        assert_eq!(entered(Step::PullRequest), 1, "and one pull request");
        assert_eq!(entered(Step::Merge), 1);
        assert_eq!(
            recorded(&events, Name::Issue(3)),
            [Happened::Started, Happened::Finished],
            "and the issue itself succeeded once"
        );

        let merged = (branch, true);
        let feature = (feature_branch(FEATURE), false);
        assert_eq!(
            hosted.pulls(),
            [merged, feature],
            "the issue's one pull request, merged, and the feature's own"
        );
        assert_eq!(hosted.state(3), State::Closed);
    }

    /// The worktree is the machine's for as long as the machine runs:
    /// the fixing agent finds the branch it is to fix still checked out,
    /// and nothing under the root survives the issue's end.
    #[tokio::test]
    async fn the_worktree_stands_through_a_fix_and_is_gone_once_the_issue_is_merged() {
        let hosted = Hosted::new("feature-standing", &[3]);
        // The fix reads what the first run left in the worktree and
        // says so, which it could only do in the worktree that holds it.
        let (mut build, mut receiver) = hosted.briefed(1, |issue, errand| match errand {
            Errand::Implement => format!("echo '{OPENS}{SESSION}'; {}", edits(issue)),
            _ => format!(
                "echo \"found $(cat issue-{issue}.txt)\"; {}",
                fixes(issue, 2)
            ),
        });
        build.required = vec![requires("test")];
        let run = tokio::spawn(build.plan(alone(3)).execute());
        let branch = issue_branch(3);
        timeout(
            PATIENCE,
            watched(&hosted, &run, &branch, |seen, tip| match seen {
                1 => hosted.red(tip),
                _ => hosted.green(tip),
            }),
        )
        .await
        .unwrap();
        timeout(PATIENCE, run).await.unwrap().unwrap().unwrap();

        let events = received(&mut receiver);
        let said = Happened::Said(text("found work"));
        assert!(
            at(&events, Name::Step(3, Step::Agent), &said).is_some(),
            "the fix ran where the first run's work was: {events:?}"
        );
        assert_eq!(hosted.state(3), State::Closed);
        let root = hosted.scratch.0.join("root");
        assert!(
            root.read_dir().unwrap().next().is_none(),
            "and no worktree outlived the issue"
        );
    }

    /// A resumed run that will not start is followed by a fresh one,
    /// briefed with the branch's diff in the session's place. The two
    /// attempts are the one run of the budget, so the issue still
    /// merges on its second.
    #[tokio::test]
    async fn a_fix_whose_session_cannot_be_resumed_runs_fresh_from_the_branchs_diff() {
        let hosted = Hosted::new("feature-fresh", &[3]);
        let (mut build, mut receiver) = hosted.briefed(1, |issue, errand| match errand {
            Errand::Implement => format!("echo '{OPENS}{SESSION}'; {}", edits(issue)),
            // The CLI refusing the session it was handed, as a script.
            Errand::Resume { .. } => "echo 'No conversation found' >&2; exit 1".to_owned(),
            Errand::Fresh { .. } => fixes(issue, 2),
        });
        build.required = vec![requires("test")];
        let run = tokio::spawn(build.plan(alone(3)).execute());
        let branch = issue_branch(3);
        timeout(
            PATIENCE,
            watched(&hosted, &run, &branch, |seen, tip| match seen {
                1 => hosted.red(tip),
                _ => hosted.green(tip),
            }),
        )
        .await
        .unwrap();
        timeout(PATIENCE, run).await.unwrap().unwrap().unwrap();

        let [implement, resume, fresh] = hosted.errands().try_into().expect("three errands");
        assert_eq!(implement, Errand::Implement);
        assert_eq!(
            resume,
            Errand::Resume {
                session: SESSION.to_owned(),
                checks: "test".to_owned(),
            }
        );
        let Errand::Fresh { diff, checks } = fresh else {
            panic!("the fix ran fresh: {fresh:?}");
        };
        assert_eq!(checks, "test");
        assert!(
            diff.contains("issue-3.txt"),
            "the branch's own diff: {diff}"
        );

        let events = received(&mut receiver);
        let agent = recorded(&events, Name::Step(3, Step::Agent));
        assert_eq!(
            agent
                .iter()
                .filter(|happened| **happened == Happened::Started)
                .count(),
            2,
            "the refused resume and the fresh run are one entry into the step: {agent:?}"
        );
        assert!(
            agent.iter().any(|happened| matches!(
                happened,
                Happened::Stderr(words) if words.contains("runs fresh")
            )),
            "and the fresh run's reason was said: {agent:?}"
        );
        let feature = (feature_branch(FEATURE), false);
        assert_eq!(hosted.pulls(), [(branch, true), feature]);
        assert_eq!(hosted.state(3), State::Closed);
    }

    #[tokio::test]
    async fn with_one_slot_an_agent_runs_while_another_issue_waits_on_its_checks() {
        // 1 and 2 start together and 3 follows 2. The checks of 2 turn
        // green once 1 is seen waiting on its own, which stay pending
        // until the agent of 3 has come and gone in the one slot.
        let hosted = Hosted::new("feature-one-slot", &[1, 2, 3]);
        let (mut build, mut receiver) = hosted.build(1, edits);
        build.required = vec![requires("test")];
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

    /// A script that leaves an edit and then waits for `flag`, so the
    /// agent holds its slot until the test raises it.
    fn paused(flag: &str) -> impl Fn(u64) -> String + Send + Sync + 'static {
        let flag = flag.to_owned();
        move |issue| {
            format!("echo work > issue-{issue}.txt; until [ -e '{flag}' ]; do sleep 0.01; done")
        }
    }

    #[tokio::test]
    async fn with_every_slot_held_by_an_agent_the_git_of_a_waiting_issue_still_runs() {
        // Four issues in one slot, every agent paused: whichever holds
        // it holds it until the flag is raised, and the three waiting
        // issues cut their worktrees meanwhile — git, and so unslotted.
        let issues = [1, 2, 3, 4];
        let hosted = Hosted::new("feature-git-unslotted", &issues);
        let flag = hosted.scratch.join("flag");
        let (build, mut receiver) = hosted.build(1, paused(&flag));
        let slots = build.slots.clone();
        let work = Dag::new(issues.map(|issue| After::new(issue, [])).into()).unwrap();
        let run = tokio::spawn(build.plan(work).execute());

        // Every worktree is cut though the one slot never comes free.
        let mut events = Vec::new();
        let cut = |events: &[Event<Name>], issue| {
            at(
                events,
                Name::Step(issue, Step::Worktree),
                &Happened::Finished,
            )
            .is_some()
        };
        timeout(PATIENCE, async {
            loop {
                sleep(Duration::from_millis(5)).await;
                events.extend(received(&mut receiver));
                if issues.iter().all(|&issue| cut(&events, issue)) {
                    break;
                }
            }
        })
        .await
        .expect("every worktree was cut while the one slot was held");
        assert_eq!(slots.0.available_permits(), 0, "an agent holds the slot");

        std::fs::write(&flag, "").unwrap();
        timeout(PATIENCE, run).await.unwrap().unwrap().unwrap();
        assert_eq!(slots.0.available_permits(), 1, "and gives it back");
    }

    #[tokio::test]
    async fn four_slots_bound_the_agents_of_every_build_that_draws_on_them() {
        // Two features of three issues each, on the host's one set of
        // four slots. Every agent pauses on the one flag, so the six
        // pile up against the four: four take a slot and two wait.
        const SLOTS: usize = 4;
        let issues = [1, 2, 3];
        let first = Hosted::new("feature-host-a", &issues);
        let second = Hosted::new("feature-host-b", &issues);
        let flag = first.scratch.join("flag");
        let slots = AgentSlots::new(NonZeroUsize::new(SLOTS).unwrap());
        let started = |hosted: &Hosted| {
            let (mut build, receiver) = hosted.build(SLOTS, paused(&flag));
            build.slots = slots.clone();
            let work = Dag::new(issues.map(|issue| After::new(issue, [])).into()).unwrap();
            (tokio::spawn(build.plan(work).execute()), receiver)
        };
        let (a, mut heard_a) = started(&first);
        let (b, mut heard_b) = started(&second);

        // The four slots fill and stay full: the fifth and sixth agents
        // wait, and no wait of theirs can take a slot that is out.
        timeout(PATIENCE, async {
            while slots.0.available_permits() > 0 {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("four agents filled the four slots");
        sleep(Duration::from_millis(100)).await;
        assert_eq!(slots.0.available_permits(), 0, "never a fifth slot");

        // Released, all six run and every slot comes back.
        std::fs::write(&flag, "").unwrap();
        let (mut ran_a, mut ran_b) = (Vec::new(), Vec::new());
        timeout(PATIENCE, async {
            while !(a.is_finished() && b.is_finished()) {
                sleep(Duration::from_millis(5)).await;
                ran_a.extend(received(&mut heard_a));
                ran_b.extend(received(&mut heard_b));
            }
        })
        .await
        .unwrap();
        timeout(PATIENCE, a).await.unwrap().unwrap().unwrap();
        timeout(PATIENCE, b).await.unwrap().unwrap().unwrap();
        ran_a.extend(received(&mut heard_a));
        ran_b.extend(received(&mut heard_b));

        for ran in [&ran_a, &ran_b] {
            for issue in issues {
                let name = Name::Step(issue, Step::Agent);
                assert!(at(ran, name, &Happened::Finished).is_some(), "{name:?}");
            }
        }
        assert_eq!(slots.0.available_permits(), SLOTS, "and gave them back");
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

        hosted.tip("issue-3").expect("the agent's commit, pushed");
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
        assert_eq!(error.to_string(), "the worktree holds nothing to commit");

        let events = received(&mut receiver);
        // The commit step read the worktree and finished on what it
        // read; having nowhere to go from there is the issue's failure.
        let words = failure(&events, Name::Issue(3));
        assert_eq!(words, "the worktree holds nothing to commit");
        assert_eq!(hosted.tip("issue-3"), None, "nothing was pushed");
        assert_eq!(hosted.pulls(), []);
    }

    #[tokio::test]
    async fn an_agent_that_left_the_worktree_off_the_issues_branch_fails_saying_where_it_is() {
        let hosted = Hosted::new("feature-wandered", &[3]);
        let (build, mut receiver) = hosted.build(1, |issue| {
            let edits = edits(issue);
            format!("git switch -qc fix; {edits}; git add . && git commit -qm 'over here'")
        });
        let error = timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the worktree is on branch fix, not on issue-3"
        );

        let events = received(&mut receiver);
        assert_eq!(
            failure(&events, Name::Step(3, Step::Commit)),
            "the worktree is on branch fix, not on issue-3"
        );
        assert_eq!(hosted.tip("issue-3"), None, "nothing was pushed");
        assert_eq!(hosted.pulls(), []);
    }

    #[tokio::test]
    async fn an_agent_that_detached_the_head_fails_rather_than_committing_off_the_branch() {
        let hosted = Hosted::new("feature-detached", &[3]);
        let (build, mut receiver) = hosted.build(1, |issue| {
            let edits = edits(issue);
            format!("git switch -q --detach; {edits}")
        });
        let error = timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the worktree is on no branch, not on issue-3"
        );

        let events = received(&mut receiver);
        let words = failure(&events, Name::Step(3, Step::Commit));
        assert_eq!(words, "the worktree is on no branch, not on issue-3");
        assert_eq!(hosted.tip("issue-3"), None, "nothing was pushed");
    }

    /// The commit crosses within the step that put it on the remote,
    /// before that step's end: a watcher hears no fact under a step
    /// that has already finished.
    #[tokio::test]
    async fn the_push_says_the_commit_it_threaded_before_it_ends() {
        let hosted = Hosted::new("feature-pushed", &[3]);
        let (build, mut receiver) = hosted.build(1, edits);
        timeout(PATIENCE, build.plan(alone(3)).execute())
            .await
            .unwrap()
            .unwrap();

        let tip = hosted.tip(&issue_branch(3)).expect("a pushed branch");
        let events = received(&mut receiver);
        assert_eq!(
            recorded(&events, Name::Step(3, Step::Push)),
            [
                Happened::Started,
                Happened::Pushed(tip.to_string()),
                Happened::Finished,
            ]
        );
    }

    /// The acceptance the monitor is for: a real build of #158's
    /// shape, every event of it recorded, folded into one watch of
    /// four issues each ending Merged at the commit its push threaded.
    /// The fold reads the build's own stream — nothing here speaks a
    /// state.
    #[tokio::test]
    async fn a_build_folds_into_one_watch_of_merged_issues() {
        use crate::monitor::{Change, Entry, Progress, RunId, State};

        let issues = [154, 155, 156, 157];
        let hosted = Hosted::new("feature-folded", &issues);
        let (build, mut receiver) = hosted.build(2, edits);
        timeout(PATIENCE, build.plan(diamond()).execute())
            .await
            .unwrap()
            .unwrap();

        // The run as a host records it: the reservation, the plan, the
        // build's whole stream, and the end.
        let run = RunId(1);
        let work = diamond();
        let titles = work
            .nodes()
            .map(|node| (*node.value(), format!("Lay stone {}", node.value())))
            .collect();
        let changes = [
            Change::Reserved {
                run,
                feature: FEATURE,
                repository: "/r".to_owned(),
                branch: feature_branch(FEATURE),
                base: Some("main".to_owned()),
            },
            Change::Started {
                run,
                work,
                title: "Build the thing".to_owned(),
                titles,
                problems: Vec::new(),
                checked: false,
            },
        ];
        let heard = received(&mut receiver);
        let spoken = heard
            .into_iter()
            .map(|event| Change::Happened { run, event });
        let changes = changes
            .into_iter()
            .chain(spoken)
            .chain([Change::Finished { run }]);

        let mut progress = Progress::default();
        for (seq, change) in changes.enumerate() {
            progress.absorb(&Entry {
                seq: seq as u64,
                at: 1_000 + seq as u64,
                change,
            });
        }

        let watch = progress.watch(run).unwrap();
        let states = watch.states();
        assert_eq!(states.len(), issues.len());
        for issue in issues {
            let tip = hosted.tip(&issue_branch(issue)).expect("a pushed branch");
            assert_eq!(
                states[&issue],
                State::Merged {
                    commit: tip.to_string(),
                    checked: false,
                },
                "#{issue} merged at the commit it pushed"
            );
        }
        let counts = watch.counts();
        assert_eq!((counts.merged, counts.total), (issues.len(), issues.len()));
        assert!(!watch.live(), "the run is over");
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

    /// The transition table on its own: every edge of the machine's
    /// diagram, decided from values alone. Nothing here cuts a
    /// worktree, calls GitHub or launches a process, which is what
    /// `2026-09-24-task-machines` split `act` from `next` for.
    mod table {
        use super::super::{Next, RUNS, State, TaskMachine, runs};
        use super::{
            Acted, AgentSlots, Build, CheckWait, FEATURE, Feature, Held, Issue, LinkedWorktree,
            Merge, Merged, Script, Sink, Step, Verdict, Worked, Worktrees,
        };
        use crate::git::commands::Hash;
        use crate::git::testing::credentials;
        use crate::git::url::Url;
        use crate::github::{GitHub, Repo};
        use std::num::NonZeroUsize;
        use std::sync::Arc;
        use std::time::Duration;

        /// The issue whose machine is asked, in a build that talks to
        /// nothing: every field is data, and none of it is reached.
        const ISSUE: u64 = 3;

        /// The commit the state carries from its push onwards.
        const PUSHED: &str = "1f0d1b0e4a5c6d7e8f90a1b2c3d4e5f607182930";

        fn issue() -> Issue<Script> {
            let (sink, _) = Sink::channel();
            let build = Build {
                feature: Feature {
                    github: GitHub::at("http://127.0.0.1:1/", None),
                    repo: Repo::new("fake", "repo"),
                    number: FEATURE,
                },
                url: Url::Local("/nowhere".into()),
                worktrees: Worktrees::new("/nowhere".into()),
                required: Vec::new(),
                checks: CheckWait {
                    every: Duration::from_millis(1),
                    bound: Duration::from_millis(1),
                },
                merge: Merge::Squash,
                slots: AgentSlots::new(NonZeroUsize::new(1).unwrap()),
                sink,
                agent: Box::new(|_, cwd, _| Script {
                    cwd: cwd.to_owned(),
                    script: String::new(),
                }),
            };
            Issue {
                number: ISSUE,
                build: Arc::new(build),
            }
        }

        /// A worktree standing in no clone, which is all the table needs
        /// of one: something the state owns and gives back.
        fn worktree() -> LinkedWorktree {
            let at = Hash::new(PUSHED).unwrap();
            LinkedWorktree::shaped("/nowhere".into(), "issue-3", at)
        }

        /// A state in `step`, `runs` agent runs spent, and whatever
        /// `arrange` makes of the rest of what it carries.
        fn state(step: Step, runs: usize, arrange: impl FnOnce(&mut Held)) -> State {
            let mut held = Held {
                worktree: worktree(),
                credentials: credentials(),
                runs,
                session: None,
                fix: None,
                pushed: Some(Hash::new(PUSHED).unwrap()),
                pull: Some(11),
            };
            arrange(&mut held);
            State::In {
                step,
                held: Box::new(held),
            }
        }

        /// Where `next` sends the machine from `state` given `outcome`,
        /// as the step it is to act in, with what it carries there.
        fn goes(state: State, outcome: Acted) -> (Step, Held) {
            match issue().next(state, outcome) {
                Next::Go(State::In { step, held }) => (step, *held),
                Next::Go(State::Cutting) => panic!("no edge goes back to the cut"),
                Next::Done => panic!("it ended"),
                Next::Fail(error) => panic!("it failed: {error:#}"),
            }
        }

        /// The words `next` failed the machine with from `state` given
        /// `outcome`.
        fn fails(state: State, outcome: Acted) -> String {
            match issue().next(state, outcome) {
                Next::Fail(error) => format!("{error:#}"),
                Next::Go(_) => panic!("it went on"),
                Next::Done => panic!("it ended well"),
            }
        }

        #[test]
        fn a_machine_starts_before_the_cut_holding_nothing() {
            assert!(matches!(issue().start(), State::Cutting));
        }

        #[test]
        fn the_cut_worktree_goes_to_the_agent_with_no_run_spent() {
            let cut = Acted::Cut(worktree(), credentials());
            let (step, held) = goes(State::Cutting, cut);
            assert_eq!(step, Step::Agent);
            assert_eq!(held.runs, 0, "the first run is yet to happen");
            assert_eq!(held.session, None);
            assert_eq!(held.fix, None, "and it implements rather than fixes");
            assert_eq!(held.pull, None);
        }

        #[test]
        fn an_agent_that_ran_goes_to_the_commit_with_its_run_counted() {
            let state = state(Step::Agent, 0, |held| held.pull = None);
            let ran = Acted::Ran(Some("s-1".to_owned()));
            let (step, held) = goes(state, ran);
            assert_eq!(step, Step::Commit);
            assert_eq!(held.runs, 1);
            assert_eq!(held.session.as_deref(), Some("s-1"));
        }

        /// The session worth resuming is the one the branch was written
        /// in, so a later run's own session does not displace it.
        #[test]
        fn a_later_run_leaves_the_session_the_first_run_opened() {
            let state = state(Step::Agent, 1, |held| {
                held.session = Some("first".to_owned());
                held.fix = Some("test".to_owned());
            });
            let (_, held) = goes(state, Acted::Ran(Some("second".to_owned())));
            assert_eq!(held.session.as_deref(), Some("first"));
            assert_eq!(held.fix, None, "and the fix it was sent on is done");
        }

        #[test]
        fn a_commit_made_goes_to_the_push_carrying_it() {
            let hash = Hash::new(PUSHED).unwrap();
            let state = state(Step::Commit, 1, |held| held.pushed = None);
            let made = Acted::Committed(Worked::At(hash.clone()));
            let (step, held) = goes(state, made);
            assert_eq!(step, Step::Push);
            assert_eq!(held.pushed, Some(hash));
        }

        #[test]
        fn a_worktree_holding_nothing_fails_the_issue() {
            let state = state(Step::Commit, 1, |_| {});
            let nothing = Acted::Committed(Worked::Nothing);
            assert_eq!(
                fails(state, nothing),
                "the worktree holds nothing to commit"
            );
        }

        #[test]
        fn a_push_with_no_pull_request_yet_goes_to_opening_one() {
            let state = state(Step::Push, 1, |held| held.pull = None);
            assert_eq!(goes(state, Acted::Pushed).0, Step::PullRequest);
        }

        #[test]
        fn a_push_onto_an_open_pull_request_goes_straight_to_its_checks() {
            let state = state(Step::Push, 2, |held| held.pull = Some(11));
            let (step, held) = goes(state, Acted::Pushed);
            assert_eq!(step, Step::Checks, "the pull request already follows it");
            assert_eq!(held.pull, Some(11), "and is still the one");
        }

        #[test]
        fn a_pull_request_opened_goes_to_its_checks_carrying_its_number() {
            let state = state(Step::PullRequest, 1, |held| held.pull = None);
            let (step, held) = goes(state, Acted::Opened(42));
            assert_eq!(step, Step::Checks);
            assert_eq!(held.pull, Some(42));
        }

        #[test]
        fn checks_that_came_back_green_go_to_the_merge() {
            let state = state(Step::Checks, 1, |_| {});
            let green = Acted::Checked(Verdict::Green);
            assert_eq!(goes(state, green).0, Step::Merge);
        }

        #[test]
        fn checks_that_claimed_no_verdict_go_to_the_merge_too() {
            let state = state(Step::Checks, 1, |_| {});
            let unclaimed = Acted::Checked(Verdict::Unclaimed);
            assert_eq!(goes(state, unclaimed).0, Step::Merge);
        }

        /// The back-edge: a red verdict with runs left returns to the
        /// agent carrying the names it is to fix, and the pull request
        /// it will come round through.
        #[test]
        fn a_red_verdict_with_runs_left_goes_back_to_the_agent_with_the_fix() {
            let state = state(Step::Checks, 1, |held| {
                held.session = Some("first".to_owned());
            });
            let red = Acted::Checked(Verdict::Red("test, clippy".to_owned()));
            let (step, held) = goes(state, red);
            assert_eq!(step, Step::Agent);
            assert_eq!(held.fix.as_deref(), Some("test, clippy"));
            assert_eq!(held.session.as_deref(), Some("first"), "to come back to");
            assert_eq!(held.pull, Some(11), "and the one pull request stands");
        }

        /// The last run the budget allows still goes round: it is the
        /// verdict after it, with none left, that fails the issue.
        #[test]
        fn a_red_verdict_on_the_next_to_last_run_still_goes_back_to_the_agent() {
            let state = state(Step::Checks, RUNS - 1, |_| {});
            let red = Acted::Checked(Verdict::Red("test".to_owned()));
            assert_eq!(goes(state, red).0, Step::Agent);
        }

        #[test]
        fn a_red_verdict_with_the_budget_spent_fails_the_issue_naming_both() {
            let state = state(Step::Checks, RUNS, |_| {});
            let red = Acted::Checked(Verdict::Red("test".to_owned()));
            assert_eq!(
                fails(state, red),
                format!("checks failed on {PUSHED}: test, after {RUNS} agent runs")
            );
        }

        /// The count the failure names reads as English whatever the
        /// budget is, so a `RUNS` of one says "1 agent run".
        #[test]
        fn a_spent_budget_is_counted_in_runs_or_in_a_run() {
            assert_eq!(runs(1), "run");
            assert_eq!(runs(RUNS), "runs");
        }

        #[test]
        fn a_merge_done_goes_to_the_closing() {
            let state = state(Step::Merge, 1, |_| {});
            let merged = Acted::Merged(Merged::Done);
            assert_eq!(goes(state, merged).0, Step::Close);
        }

        #[test]
        fn a_merge_refused_fails_the_issue_in_githubs_own_words() {
            let state = state(Step::Merge, 1, |_| {});
            let refused = Acted::Merged(Merged::Refused("not mergeable".to_owned()));
            let words = fails(state, refused);
            assert!(words.contains("not mergeable"), "{words}");
        }

        #[test]
        fn the_closing_ends_the_machine() {
            let state = state(Step::Close, 1, |_| {});
            assert!(matches!(issue().next(state, Acted::Closed), Next::Done));
        }

        /// The one state the machine has that the table cannot place an
        /// outcome in is an outcome of another state's: it is said as
        /// the fact it is rather than taken for one of the edges.
        #[test]
        fn an_outcome_of_another_state_fails_the_machine_naming_the_state() {
            let state = state(Step::Merge, 1, |_| {});
            let words = fails(state, Acted::Closed);
            assert_eq!(words, "Merge came to an outcome of another step's");
        }

        /// A worktree is the state's to hold and the state's to let go
        /// of: an edge onwards keeps it, and the end of the machine —
        /// here, the closing — drops it. `Machine` is what drops the
        /// last state; the table hands it back.
        #[test]
        fn a_state_going_on_carries_its_worktree_and_who_its_git_answers_as() {
            let state = state(Step::Push, 1, |_| {});
            let (_, held) = goes(state, Acted::Pushed);
            assert_eq!(held.worktree.branch(), "issue-3");
            assert_eq!(held.credentials.username, credentials().username);
        }
    }
}
