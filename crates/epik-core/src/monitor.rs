//! A feature build, watched: the stream the build speaks, recorded as
//! an append-only log, and the fold that turns the log back into the
//! picture the build holds.
//!
//! A build says what it is doing on one stream of
//! [`Event<Name>`](Event): [`Happened`] at a node of its plan, or at
//! one [`Step`] of an issue's node. That stream is the record, and an
//! issue's state is derived from it rather than spoken alongside it —
//! where the two could disagree, there is only one.
//!
//! Every event is recorded as an [`Entry`] in the host's one [`Log`],
//! beside the handful of run-level [`Change`]s the stream does not
//! carry — the reservation before a launch, the plan the build runs,
//! and abandonment — and anything that wants the picture folds the
//! entries into a [`Progress`]: every build the host has heard of,
//! each a [`Watch`]. Attaching is replay, not a snapshot: there is one
//! representation on the wire and one fold, so the document a watcher
//! gets on attach cannot disagree with the one it builds by listening.
//! The log is bounded by the plan, not by time — a build makes a
//! handful of entries per issue and then stops — which is what makes
//! replay affordable.
//!
//! [`Entry::seq`] is host-wide and monotonic from 0, and is three
//! things at once by design: the deduplication key, the replay cursor,
//! and the id an event stream carries. [`Entry::at`] is unix
//! milliseconds, stamped by the host — the one field the wasm-clean
//! half cannot produce for itself. [`Log`] is where both are assigned,
//! and the only place either is; it alone needs the `native` feature,
//! so the window folds this module's fold rather than a copy of it.

pub mod layout;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dag::Dag;
use crate::happening::{Happened, Name, Step};
use layout::Layout;

/// The event channel entries arrive on, backend to window.
pub const EVENT: &str = "monitor";

/// The key of the feature-build record: one per launched build,
/// counting up from 1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RunId(pub u64);

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where one issue stands, as the stream says: waiting on a blocker or
/// a slot, running under an agent, merging once the agent is done, and
/// three ends — landed, failed in its own right, or skipped because a
/// blocker failed and nothing this build can do would make it ready.
///
/// Derived from the stream, never spoken into it: the step an issue's
/// node is in says which of these it is.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Not yet started: blocked, or ready and out of slots.
    Waiting,
    /// An agent holds a slot and is working the issue's branch: the
    /// worktree, the agent, the commit, the push.
    Running,
    /// The agent is done and the slot released; the pull request is
    /// open, and what remains is GitHub's — the checks, the merge, the
    /// close.
    Merging,
    /// Merged onto the feature branch and closed. `commit` is the hash
    /// the push threaded; `checked` is whether checks were expected on
    /// the pull request, GitHub being what runs them.
    Merged { commit: String, checked: bool },
    /// Failed, in the words of the step that failed.
    Failed { report: String },
    /// Never started, and never will be: the issue whose failure
    /// stopped it, which this build can never settle. The number, not
    /// words about it — a record keeps facts, and the pane says them.
    Skipped { blocker: u64 },
}

/// How far an issue has got, by the stream's account. An issue the
/// stream has said nothing of has no `Progressed` at all, so merely
/// having one means its node has spoken.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Progressed {
    /// The last step to have spoken.
    step: Option<Step>,
    /// The commit the push threaded, once it has.
    commit: Option<String>,
    /// The words of the step that failed, once one has.
    failed: Option<String>,
    /// Whether the close finished: the issue is landed.
    closed: bool,
}

impl Progressed {
    /// Where the issue stands: the step it is in, read as a state.
    /// Failure wins over everything, the close finishing is the end,
    /// and otherwise the step says whether an agent holds a slot.
    fn state(&self, checked: bool) -> State {
        if let Some(report) = &self.failed {
            return State::Failed {
                report: report.clone(),
            };
        }
        if self.closed {
            return State::Merged {
                commit: self.commit.clone().unwrap_or_default(),
                checked,
            };
        }
        // The steps are declared in the order they run, so one
        // comparison divides the agent's half from GitHub's. A node
        // that has spoken without reaching a step is under way.
        match self.step {
            Some(step) if step > Step::Push => State::Merging,
            _ => State::Running,
        }
    }
}

/// One thing that happened to a feature build that its own stream does
/// not carry: the run's life, spoken by whatever starts it. Reserved,
/// then Started, then Finished — or Abandoned in place of Started,
/// when the launch failed between reserving and running.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// A run id is held for a launch: recorded before the check card is
    /// raised, so a build is visible from the moment it is asked for.
    Reserved {
        run: RunId,
        feature: u64,
        repository: String,
        branch: String,
        base: Option<String>,
    },
    /// The build is running: the work it runs and the order among it,
    /// the feature's title and its issues', what is wrong with the
    /// plan's shape in words, and whether checks are expected on each
    /// pull request.
    ///
    /// The issues' titles cross as pairs rather than a map, because a
    /// JSON object's keys are strings and this enum is tagged — which
    /// buffers what it reads, and so cannot read `"154"` back as a
    /// number the way a plain map would.
    Started {
        run: RunId,
        work: Dag<u64>,
        title: String,
        titles: Vec<(u64, String)>,
        problems: Vec<String>,
        checked: bool,
    },
    /// The build's own stream, one event of it.
    Happened { run: RunId, event: Event },
    /// Nothing is ready and nothing is running: every state is terminal.
    Finished { run: RunId },
    /// The launch failed after reserving, or the build was given up;
    /// `reason` is why, in words.
    Abandoned { run: RunId, reason: String },
}

/// One event of a build's stream, as the log keeps it.
pub type Event = crate::happening::Event<Name>;

impl Change {
    /// The run this change belongs to.
    #[must_use]
    pub const fn run(&self) -> RunId {
        match self {
            Self::Reserved { run, .. }
            | Self::Started { run, .. }
            | Self::Happened { run, .. }
            | Self::Finished { run }
            | Self::Abandoned { run, .. } => *run,
        }
    }
}

/// A change as the log keeps it: stamped with its place in the log and
/// the moment it was recorded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Entry {
    /// Host-wide, monotonic from 0: the deduplication key, the replay
    /// cursor, and the id an event stream carries.
    pub seq: u64,
    /// Unix milliseconds, as the host's clock read when the change was
    /// recorded.
    pub at: u64,
    pub change: Change,
}

/// Where one run stands in its life.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Reserved: the launch is validating, asking, establishing.
    Starting,
    /// Started, and not yet finished.
    Building,
    /// Every state is terminal.
    Finished,
    /// Given up, and why.
    Abandoned { reason: String },
}

/// How many issues stand where. `ready` and `blocked` split Waiting: a
/// Waiting issue is ready when every issue before it has merged, and
/// blocked otherwise. `total` is every issue of work.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    pub merged: usize,
    pub running: usize,
    pub merging: usize,
    pub failed: usize,
    pub skipped: usize,
    pub ready: usize,
    pub blocked: usize,
    pub total: usize,
}

/// One feature build as a watcher sees it: what Reserved said of it,
/// what Started handed over, and every issue's state as the build's own
/// stream leaves it. Obtained only by folding — [`Progress::absorb`] is
/// the one way in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Watch {
    run: RunId,
    feature: u64,
    repository: String,
    branch: String,
    base: Option<String>,
    /// `None` until Started: a reservation has no work yet.
    work: Option<Dag<u64>>,
    title: Option<String>,
    titles: BTreeMap<u64, String>,
    problems: Vec<String>,
    checked: bool,
    /// How far each issue has got, by the stream's account.
    progressed: BTreeMap<u64, Progressed>,
    stage: Stage,
}

impl Watch {
    fn reserved(
        run: RunId,
        feature: u64,
        repository: String,
        branch: String,
        base: Option<String>,
    ) -> Self {
        Self {
            run,
            feature,
            repository,
            branch,
            base,
            work: None,
            title: None,
            titles: BTreeMap::new(),
            problems: Vec::new(),
            checked: false,
            progressed: BTreeMap::new(),
            stage: Stage::Starting,
        }
    }

    #[must_use]
    pub const fn run(&self) -> RunId {
        self.run
    }

    #[must_use]
    pub const fn feature(&self) -> u64 {
        self.feature
    }

    #[must_use]
    pub fn repository(&self) -> &str {
        &self.repository
    }

    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    #[must_use]
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// The work the build runs, once Started has said what it is.
    #[must_use]
    pub const fn work(&self) -> Option<&Dag<u64>> {
        self.work.as_ref()
    }

    /// The feature's title, once Started has handed it over.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The work as a picture, every issue drawn where it stands — or
    /// nothing, before there is any.
    #[must_use]
    pub fn layout(&self) -> Option<Layout> {
        self.work
            .as_ref()
            .map(|work| layout::layout(work, &self.titles, &self.states()))
    }

    /// What is wrong with the plan's shape, in words.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Every issue of work and where it stands, derived from the
    /// stream. An issue the stream has said nothing of is Waiting; one
    /// whose predecessor failed is Skipped, naming the blocker.
    #[must_use]
    pub fn states(&self) -> BTreeMap<u64, State> {
        let Some(work) = &self.work else {
            return BTreeMap::new();
        };
        let mut states: BTreeMap<u64, State> = BTreeMap::new();
        // The graph's order puts every predecessor before the node it
        // precedes, so one pass settles every state: a node reads what
        // is already settled behind it, and the issue that failed
        // carries its own number forward from skip to skip.
        for node in work.nodes() {
            let id = *node.value();
            let state = match self.progressed.get(&id) {
                Some(progressed) => progressed.state(self.checked),
                // A node the stream has not reached, held back by a
                // blocker that will never settle, is skipped in the
                // name of the failure that stopped it: the plan runs
                // nothing after one.
                None => node
                    .predecessors()
                    .find_map(|before| match states.get(before.value())? {
                        State::Failed { .. } => Some(*before.value()),
                        State::Skipped { blocker } => Some(*blocker),
                        _ => None,
                    })
                    .map_or(State::Waiting, |blocker| State::Skipped { blocker }),
            };
            states.insert(id, state);
        }
        states
    }

    #[must_use]
    pub const fn stage(&self) -> &Stage {
        &self.stage
    }

    /// Whether an agent holds a slot right now: some issue is Running.
    /// Merging is not live — no agent holds a slot while GitHub runs
    /// the checks — so a pane that shows motion for the present shows
    /// it only here.
    #[must_use]
    pub fn live(&self) -> bool {
        // Only an issue the stream has spoken of can be Running, so
        // this asks what it has said and never walks the graph.
        self.progressed
            .values()
            .any(|progressed| matches!(progressed.state(self.checked), State::Running))
    }

    /// How many issues stand where: the picture's nodes tallied by
    /// [`Form`](layout::Form), which is where Waiting is split into
    /// ready and blocked by what has merged.
    #[must_use]
    pub fn counts(&self) -> Counts {
        use layout::Form;
        let mut counts = Counts::default();
        for node in self.layout().iter().flat_map(|layout| &layout.nodes) {
            counts.total += 1;
            match node.form {
                Form::Ready => counts.ready += 1,
                Form::Blocked => counts.blocked += 1,
                Form::Running => counts.running += 1,
                Form::Merging => counts.merging += 1,
                Form::Merged => counts.merged += 1,
                Form::Failed => counts.failed += 1,
                Form::Skipped => counts.skipped += 1,
            }
        }
        counts
    }

    /// One change about this run, taken in. Borrowed: the stream's
    /// commonest event by far is an agent's narration, which moves
    /// nothing, so the fold clones only what it keeps.
    fn apply(&mut self, change: &Change) {
        match change {
            Change::Reserved { .. } => {}
            Change::Started {
                work,
                title,
                titles,
                problems,
                checked,
                ..
            } => {
                self.work = Some(work.clone());
                self.title = Some(title.clone());
                self.titles = titles.iter().cloned().collect();
                self.problems = problems.clone();
                self.checked = *checked;
                self.stage = Stage::Building;
            }
            Change::Happened { event, .. } => self.heard(event),
            Change::Finished { .. } => self.stage = Stage::Finished,
            Change::Abandoned { reason, .. } => {
                self.stage = Stage::Abandoned {
                    reason: reason.clone(),
                };
            }
        }
    }

    /// One event of the build's stream, taken in. Only what an issue's
    /// node and its steps say moves a state; the branch's and the pull
    /// request's nodes are the build's own doing, not any issue's.
    fn heard(&mut self, event: &Event) {
        let (issue, step) = match event.node {
            Name::Issue(issue) => (issue, None),
            Name::Step(issue, step) => (issue, Some(step)),
            Name::Branch | Name::PullRequest => return,
        };
        // What an agent says is for the transcript, not the state, and
        // it is most of what crosses: turned away before a node is
        // made for an issue whose only traffic so far is narration.
        if matches!(event.happened, Happened::Said(_) | Happened::Stderr(_)) {
            return;
        }
        let progressed = self.progressed.entry(issue).or_default();
        match (&event.happened, step) {
            // A failure is the issue's, in the words of whichever step
            // failed: the first to fail is the one that stopped it.
            (Happened::Failed(report), _) => {
                progressed.failed.get_or_insert_with(|| report.clone());
            }
            (Happened::Started, Some(step)) => progressed.step = Some(step),
            (Happened::Finished, Some(step)) => {
                progressed.step = Some(step);
                progressed.closed = step == Step::Close;
            }
            (Happened::Pushed(commit), Some(Step::Push)) => {
                progressed.commit = Some(commit.clone());
            }
            // The issue's own node: its start puts it Running before
            // its first step speaks, and its end is already told by
            // the close finishing.
            (Happened::Started, None) => {}
            (Happened::Finished | Happened::Pushed(_), _) => {}
            (Happened::Said(_) | Happened::Stderr(_), _) => unreachable!("turned away above"),
        }
    }
}

/// Every feature build the host has heard of, folded from the log's
/// entries and from nothing else. Fresh, it has heard of none; absorb
/// the replay, then keep absorbing what arrives.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    /// The highest `seq` applied, once any has been.
    applied: Option<u64>,
    watches: BTreeMap<RunId, Watch>,
}

impl Progress {
    /// Folds one entry in, and says whether it counted. An entry whose
    /// `seq` is not past the highest already applied is ignored — the
    /// same entry heard twice, or a replay overlapping what was already
    /// listened to, changes nothing — which is what makes attaching
    /// safe in either order: listen then replay, or replay then listen.
    /// A change to a run whose Reserved this fold never heard has
    /// nowhere to land and is dropped; the replay from 0 is what makes
    /// a fold complete.
    pub fn absorb(&mut self, entry: &Entry) -> bool {
        if self.applied.is_some_and(|applied| entry.seq <= applied) {
            return false;
        }
        self.applied = Some(entry.seq);
        match &entry.change {
            Change::Reserved {
                run,
                feature,
                repository,
                branch,
                base,
            } => {
                let watch = Watch::reserved(
                    *run,
                    *feature,
                    repository.clone(),
                    branch.clone(),
                    base.clone(),
                );
                self.watches.insert(*run, watch);
            }
            change => {
                if let Some(watch) = self.watches.get_mut(&change.run()) {
                    watch.apply(change);
                }
            }
        }
        true
    }

    /// The highest `seq` applied — the cursor to resume from — or `None`
    /// before anything has been.
    #[must_use]
    pub const fn applied(&self) -> Option<u64> {
        self.applied
    }

    #[must_use]
    pub fn watch(&self, run: RunId) -> Option<&Watch> {
        self.watches.get(&run)
    }

    /// Every build heard of, oldest first.
    pub fn watches(&self) -> impl Iterator<Item = &Watch> {
        self.watches.values()
    }
}

#[cfg(feature = "native")]
pub use log::{Log, now};

#[cfg(feature = "native")]
mod log {
    use std::fmt;
    use std::sync::{Condvar, Mutex, PoisonError};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{Change, Entry};

    /// The host's one log of feature-build changes: where a `seq` and an
    /// `at` are assigned, and the only place either is. Readers replay
    /// with [`since`](Self::since) and follow with
    /// [`wait_after`](Self::wait_after). A cursor is a count: how many
    /// entries the caller has applied, which — `seq` being dense from 0
    /// — is also the next `seq` it wants.
    ///
    /// The log never calls anyone: delivery is a reader's, and a reader
    /// that advances its own cursor hears every entry in `seq` order by
    /// construction, however many threads are recording. A log that
    /// called back on record could not promise that — the callback for
    /// `seq` N+1 can run before the one for N — and a fold keyed on
    /// `seq` would drop N for good.
    pub struct Log {
        entries: Mutex<Vec<Entry>>,
        appended: Condvar,
        /// Unix milliseconds now. Injected, so a test asserts the stamps
        /// it chose without sleeping.
        clock: Box<dyn Fn() -> u64 + Send + Sync>,
    }

    impl fmt::Debug for Log {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            f.debug_struct("Log")
                .field("entries", &entries.len())
                .finish_non_exhaustive()
        }
    }

    impl Default for Log {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Unix milliseconds now, as the host's clock reads: what a log on
    /// the system clock stamps [`Entry::at`] with, and the one reading
    /// of the clock anything beside the log takes.
    #[must_use]
    pub fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }

    impl Log {
        /// An empty log on the system clock.
        #[must_use]
        pub fn new() -> Self {
            Self::with_clock(now)
        }

        /// An empty log stamping entries with whatever `clock` answers.
        #[must_use]
        pub fn with_clock(clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                appended: Condvar::new(),
                clock: Box::new(clock),
            }
        }

        /// Records `change`: the next `seq`, the clock's stamp, and
        /// every waiting reader woken.
        pub fn record(&self, change: Change) {
            {
                let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
                let entry = Entry {
                    seq: entries.len() as u64,
                    at: (self.clock)(),
                    change,
                };
                entries.push(entry);
            }
            self.appended.notify_all();
        }

        /// Every entry from `cursor` on: `since(0)` is the whole log, the
        /// replay a watcher attaches with.
        #[must_use]
        pub fn since(&self, cursor: u64) -> Vec<Entry> {
            let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            tail(&entries, cursor)
        }

        /// [`since`](Self::since), waited for: the entries from `cursor`
        /// on as soon as there is one, or none when `timeout` passes
        /// first — the long poll an event stream is served from.
        #[must_use]
        pub fn wait_after(&self, cursor: u64, timeout: Duration) -> Vec<Entry> {
            let start = usize::try_from(cursor).unwrap_or(usize::MAX);
            let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            let (entries, _) = self
                .appended
                .wait_timeout_while(entries, timeout, |entries| entries.len() <= start)
                .unwrap_or_else(PoisonError::into_inner);
            tail(&entries, cursor)
        }
    }

    /// The entries from `cursor` on; none when the cursor is past the end.
    fn tail(entries: &[Entry], cursor: u64) -> Vec<Entry> {
        entries
            .get(usize::try_from(cursor).unwrap_or(usize::MAX)..)
            .map(<[Entry]>::to_vec)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::Node as After;

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

    pub(super) fn reserved(run: u64) -> Change {
        Change::Reserved {
            run: RunId(run),
            feature: 158,
            repository: "/r".to_owned(),
            branch: "feature-158".to_owned(),
            base: Some("main".to_owned()),
        }
    }

    fn started(run: u64, work: Dag<u64>) -> Change {
        let titles: Vec<(u64, String)> = work
            .nodes()
            .map(|node| (*node.value(), format!("Lay stone {}", node.value())))
            .collect();
        Change::Started {
            run: RunId(run),
            work,
            title: "Build the thing".to_owned(),
            titles,
            problems: Vec::new(),
            checked: false,
        }
    }

    /// One `Change::Happened`: what happened at `node` in `run`.
    fn change(run: u64, node: Name, happened: Happened) -> Change {
        Change::Happened {
            run: RunId(run),
            event: Event { node, happened },
        }
    }

    /// The whole of one issue's run, start to close, as its node and
    /// its steps say it.
    fn built(run: u64, issue: u64) -> Vec<Change> {
        let mut changes = vec![change(run, Name::Issue(issue), Happened::Started)];
        for step in [
            Step::Worktree,
            Step::Agent,
            Step::Commit,
            Step::Push,
            Step::PullRequest,
            Step::Checks,
            Step::Merge,
            Step::Close,
        ] {
            let node = Name::Step(issue, step);
            changes.push(change(run, node, Happened::Started));
            if step == Step::Push {
                let commit = format!("commit-{issue}");
                changes.push(change(run, node, Happened::Pushed(commit)));
            }
            changes.push(change(run, node, Happened::Finished));
        }
        changes.push(change(run, Name::Issue(issue), Happened::Finished));
        changes
    }

    /// The changes folded, in order, from seq 0.
    fn fold(changes: &[Change]) -> Progress {
        let mut progress = Progress::default();
        for (seq, change) in changes.iter().enumerate() {
            let entry = Entry {
                seq: seq as u64,
                at: 1_000 + seq as u64,
                change: change.clone(),
            };
            assert!(progress.absorb(&entry), "entry {seq} counted");
        }
        progress
    }

    /// A whole build of #158's shape: every issue built, then finished.
    fn whole_build() -> Vec<Change> {
        let mut changes = vec![reserved(1), started(1, diamond())];
        changes.push(change(1, Name::Branch, Happened::Started));
        changes.push(change(1, Name::Branch, Happened::Finished));
        for issue in [154, 155, 156, 157] {
            changes.extend(built(1, issue));
        }
        changes.push(change(1, Name::PullRequest, Happened::Started));
        changes.push(change(1, Name::PullRequest, Happened::Finished));
        changes.push(Change::Finished { run: RunId(1) });
        changes
    }

    /// A build of #158's shape yields one watch of four issues, each
    /// ending Merged at the commit its push threaded, the run Finished.
    #[test]
    fn a_whole_build_folds_to_four_merged_issues_and_a_finished_run() {
        let progress = fold(&whole_build());
        assert_eq!(progress.watches().count(), 1);
        let watch = progress.watch(RunId(1)).unwrap();
        assert_eq!(*watch.stage(), Stage::Finished);
        assert_eq!(watch.feature(), 158);
        assert_eq!(watch.title(), Some("Build the thing"));

        let states = watch.states();
        assert_eq!(states.len(), 4);
        for issue in [154, 155, 156, 157] {
            assert_eq!(
                states[&issue],
                State::Merged {
                    commit: format!("commit-{issue}"),
                    checked: false,
                },
                "#{issue}"
            );
        }
        let counts = watch.counts();
        assert_eq!((counts.merged, counts.total), (4, 4));
        assert!(!watch.live(), "nothing is running at the end");
    }

    /// An issue whose agent has exited and whose pull request is open
    /// reads Merging: no agent holds a slot while GitHub runs.
    #[test]
    fn an_issue_waiting_on_github_is_merging_not_running() {
        let mut changes = vec![reserved(1), started(1, diamond())];
        changes.push(change(1, Name::Issue(154), Happened::Started));
        for step in [Step::Worktree, Step::Agent, Step::Commit, Step::Push] {
            let node = Name::Step(154, step);
            changes.push(change(1, node, Happened::Started));
            changes.push(change(1, node, Happened::Finished));
        }
        let running = fold(&changes);
        assert_eq!(
            running.watch(RunId(1)).unwrap().states()[&154],
            State::Running,
            "the push is still the agent's side of the work"
        );
        assert!(
            running.watch(RunId(1)).unwrap().live(),
            "the run has not finished"
        );

        let node = Name::Step(154, Step::PullRequest);
        changes.push(change(1, node, Happened::Started));
        let merging = fold(&changes);
        let watch = merging.watch(RunId(1)).unwrap();
        assert_eq!(watch.states()[&154], State::Merging);
        assert!(!watch.live(), "no agent holds a slot during a merge");
        assert_eq!(watch.counts().merging, 1);
    }

    /// A failed step fails its issue in that step's words, and the
    /// nodes it held back read Skipped naming the blocker.
    #[test]
    fn a_failed_step_fails_its_issue_and_skips_what_it_held_back() {
        let words = "the agent exited with code 3";
        let mut changes = vec![reserved(1), started(1, diamond())];
        changes.extend(built(1, 154));
        changes.push(change(1, Name::Issue(155), Happened::Started));
        for (step, ended) in [
            (Step::Worktree, Happened::Finished),
            (Step::Agent, Happened::Failed(words.to_owned())),
        ] {
            let node = Name::Step(155, step);
            changes.push(change(1, node, Happened::Started));
            changes.push(change(1, node, ended));
        }
        changes.push(change(
            1,
            Name::Issue(155),
            Happened::Failed(words.to_owned()),
        ));
        changes.push(Change::Finished { run: RunId(1) });

        let progress = fold(&changes);
        let watch = progress.watch(RunId(1)).unwrap();
        let states = watch.states();
        assert!(
            matches!(states[&154], State::Merged { .. }),
            "{:?}",
            states[&154]
        );
        assert_eq!(
            states[&155],
            State::Failed {
                report: words.to_owned()
            },
            "in the step's own words"
        );
        for held in [156, 157] {
            assert_eq!(
                states[&held],
                State::Skipped { blocker: 155 },
                "#{held} waited on 155",
            );
        }
        let counts = watch.counts();
        assert_eq!((counts.merged, counts.failed, counts.skipped), (1, 1, 2));
    }

    /// A watcher attaching mid-build and replaying from 0 holds the
    /// same Progress as one that listened from the start.
    #[test]
    fn replaying_from_zero_holds_what_listening_from_the_start_does() {
        let changes = whole_build();
        let listened = fold(&changes);

        // The same entries, taken in two goes with an overlap: a
        // watcher that heard the tail live, then replayed the whole
        // log behind it.
        let entries: Vec<Entry> = changes
            .iter()
            .enumerate()
            .map(|(seq, change)| Entry {
                seq: seq as u64,
                at: 1_000 + seq as u64,
                change: change.clone(),
            })
            .collect();
        let mut attached = Progress::default();
        for entry in &entries[entries.len() - 3..] {
            attached.absorb(entry);
        }
        let mut replayed = Progress::default();
        for entry in &entries {
            replayed.absorb(entry);
        }
        assert_eq!(replayed, listened);

        // And the other order: replay first, then the live tail, which
        // the cursor makes a no-op.
        let mut both = replayed.clone();
        for entry in &entries[entries.len() - 3..] {
            assert!(!both.absorb(entry), "already applied");
        }
        assert_eq!(both, listened);
    }

    #[test]
    fn an_entry_not_past_the_highest_applied_is_ignored() {
        let changes = [reserved(1), started(1, diamond())];
        let mut progress = fold(&changes);
        let once = progress.clone();
        let repeat = Entry {
            seq: 1,
            at: 1_001,
            change: changes[1].clone(),
        };
        assert!(
            !progress.absorb(&repeat),
            "a seq already applied changes nothing"
        );
        assert_eq!(progress, once);
        assert_eq!(progress.applied(), Some(1));
    }

    #[test]
    fn a_change_to_a_run_never_reserved_has_nowhere_to_land() {
        let mut progress = Progress::default();
        let entry = Entry {
            seq: 0,
            at: 1_000,
            change: Change::Finished { run: RunId(9) },
        };
        assert!(
            progress.absorb(&entry),
            "a finish for an unknown run still applies"
        );
        assert_eq!(progress.applied(), Some(0), "the cursor still moved");
        assert!(
            progress.watch(RunId(9)).is_none(),
            "run 9 was never started"
        );
    }

    #[test]
    fn a_reservation_has_no_work_and_no_picture_yet() {
        let progress = fold(&[reserved(1)]);
        let watch = progress.watch(RunId(1)).unwrap();
        assert_eq!(*watch.stage(), Stage::Starting);
        assert_eq!(watch.branch(), "feature-158");
        assert_eq!(watch.base(), Some("main"));
        assert_eq!(watch.repository(), "/r");
        assert!(watch.work().is_none(), "a reservation has no graph yet");
        assert_eq!(watch.title(), None);
        assert_eq!(watch.layout(), None);
        assert!(watch.states().is_empty(), "and so no node has a state");
        assert_eq!(watch.counts(), Counts::default());
    }

    #[test]
    fn an_abandoned_launch_keeps_its_reason() {
        let progress = fold(&[
            reserved(1),
            Change::Abandoned {
                run: RunId(1),
                reason: "no plan".to_owned(),
            },
        ]);
        assert_eq!(
            *progress.watch(RunId(1)).unwrap().stage(),
            Stage::Abandoned {
                reason: "no plan".to_owned()
            }
        );
    }

    /// Waiting splits into ready and blocked by what has merged, and
    /// the split moves as an issue lands.
    #[test]
    fn counts_split_waiting_into_ready_and_blocked() {
        let tally = |progress: &Progress| {
            let counts = progress.watch(RunId(1)).unwrap().counts();
            (counts.merged, counts.ready, counts.blocked, counts.total)
        };
        let mut changes = vec![reserved(1), started(1, diamond())];
        assert_eq!(tally(&fold(&changes)), (0, 2, 2, 4), "154 and 155 are open");

        changes.extend(built(1, 154));
        assert_eq!(
            tally(&fold(&changes)),
            (1, 1, 2, 4),
            "156 still waits on 155"
        );
        changes.extend(built(1, 155));
        assert_eq!(
            tally(&fold(&changes)),
            (2, 1, 1, 4),
            "155 landing released 156"
        );
    }

    /// The picture: a column per step of the order, a row per issue,
    /// an arrow per edge, and the titles Started handed over.
    #[test]
    fn the_layout_draws_the_work_left_to_right() {
        let progress = fold(&[reserved(1), started(1, diamond())]);
        let layout = progress.watch(RunId(1)).unwrap().layout().unwrap();
        let cells: Vec<(u64, usize, usize)> = layout
            .nodes
            .iter()
            .map(|node| (node.id, node.column, node.row))
            .collect();
        assert_eq!(cells, [(154, 0, 0), (155, 0, 1), (156, 1, 2), (157, 2, 3)]);
        let arrows: Vec<(u64, u64)> = layout
            .edges
            .iter()
            .map(|edge| (edge.blocker, edge.issue))
            .collect();
        assert_eq!(arrows, [(154, 156), (155, 156), (156, 157)]);
        assert_eq!((layout.columns(), layout.rows()), (3, 4));
        assert_eq!(layout.node(154).unwrap().title, "Lay stone 154");
        assert_eq!(layout.node(156).unwrap().blocked_by, [154, 155]);
        assert_eq!(layout.node(154).unwrap().blocks, [156]);
        assert!(
            layout.containers.is_empty(),
            "flat work carries no containment"
        );
    }

    #[test]
    fn a_change_serializes_tagged_by_its_word_and_reads_back() {
        let pushed = change(1, Name::Step(154, Step::Push), Happened::Finished);
        let wire = serde_json::to_value(&pushed).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "change": "happened",
                "run": 1,
                "event": { "node": { "Step": [154, "Push"] }, "happened": "Finished" },
            })
        );
        assert_eq!(serde_json::from_value::<Change>(wire).unwrap(), pushed);

        let entry = Entry {
            seq: 3,
            at: 1_003,
            change: started(1, diamond()),
        };
        let line = serde_json::to_string(&entry).unwrap();
        assert_eq!(serde_json::from_str::<Entry>(&line).unwrap(), entry);
    }

    /// Checks expected on the pull request is what `checked` says now,
    /// GitHub being what runs them.
    #[test]
    fn checked_says_whether_checks_were_expected() {
        let mut changes = vec![reserved(1), started(1, diamond())];
        if let Change::Started { checked, .. } = &mut changes[1] {
            *checked = true;
        }
        changes.extend(built(1, 154));
        let progress = fold(&changes);
        assert_eq!(
            progress.watch(RunId(1)).unwrap().states()[&154],
            State::Merged {
                commit: "commit-154".to_owned(),
                checked: true,
            }
        );
    }
}

/// The log, which only a host keeps: recorded from many threads and
/// read by cursor, over the same changes the fold above takes in. Named
/// `native` for its gate, the feature the log lives behind.
#[cfg(all(test, feature = "native"))]
mod native {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::tests::reserved;
    use super::*;

    #[test]
    fn the_log_stamps_seq_and_the_injected_clocks_time() {
        let now = Arc::new(Mutex::new(5_000_u64));
        let log = Log::with_clock({
            let now = Arc::clone(&now);
            move || *now.lock().unwrap()
        });
        log.record(reserved(1));
        *now.lock().unwrap() = 5_250;
        log.record(Change::Finished { run: RunId(1) });

        let entries = log.since(0);
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].seq, entries[0].at), (0, 5_000));
        assert_eq!((entries[1].seq, entries[1].at), (1, 5_250));
        assert_eq!(log.since(1), entries[1..]);
        assert!(log.since(2).is_empty(), "the cursor is past the last entry");
        assert!(log.since(u64::MAX).is_empty(), "and far past it");
    }

    #[test]
    fn wait_after_times_out_empty_and_wakes_on_a_record() {
        let log = Arc::new(Log::new());
        assert!(
            log.wait_after(0, Duration::from_millis(20)).is_empty(),
            "nothing came"
        );

        let waiter = std::thread::spawn({
            let log = Arc::clone(&log);
            move || log.wait_after(0, Duration::from_secs(30))
        });
        log.record(reserved(1));
        let heard = waiter.join().unwrap();
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0].change, reserved(1));

        // Already satisfied: answers at once with everything from the cursor.
        assert_eq!(log.wait_after(0, Duration::from_secs(30)), heard);
        assert!(
            log.wait_after(1, Duration::from_millis(20)).is_empty(),
            "nothing arrives inside the wait"
        );
    }

    /// Two runs recording at once, and one reader following by
    /// cursor: every entry reaches it, in `seq` order, none twice —
    /// the ordering a callback on record could not promise.
    #[test]
    fn a_reader_following_by_cursor_hears_concurrent_records_in_seq_order() {
        const EACH: u64 = 200;
        let log = Arc::new(Log::new());
        let reader = std::thread::spawn({
            let log = Arc::clone(&log);
            move || {
                let mut heard = Vec::new();
                while heard.len() < (2 * EACH) as usize {
                    let entries = log.wait_after(heard.len() as u64, Duration::from_secs(30));
                    assert!(!entries.is_empty(), "the recorders went quiet");
                    heard.extend(entries);
                }
                heard
            }
        });
        let recorders: Vec<_> = [1, 2]
            .into_iter()
            .map(|run| {
                let log = Arc::clone(&log);
                std::thread::spawn(move || {
                    for _ in 0..EACH {
                        log.record(reserved(run));
                    }
                })
            })
            .collect();
        for recorder in recorders {
            recorder.join().unwrap();
        }

        let heard = reader.join().unwrap();
        let seqs: Vec<u64> = heard.iter().map(|entry| entry.seq).collect();
        assert_eq!(seqs, (0..2 * EACH).collect::<Vec<_>>());
        assert_eq!(heard, log.since(0));
    }
}
