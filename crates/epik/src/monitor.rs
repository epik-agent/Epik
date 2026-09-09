//! A feature build, spoken: an append-only log of what changed, and the
//! fold that turns the log back into the picture the build holds.
//!
//! [`Build`](crate::feature::Build) keeps one [`State`] per issue of
//! work and moves it as the build proceeds. Every movement is a
//! [`Change`], every change is recorded as an [`Entry`] in the host's
//! one [`Log`], and anything that wants the picture — a pane in the
//! window, a browser at the far end of an event stream — folds the
//! entries into a [`Progress`]: every build the host has heard of, each
//! a [`Watch`]. Attaching is replay, not a snapshot: there is one
//! representation on the wire and one fold, so the document a watcher
//! gets on attach cannot disagree with the one it builds by listening.
//! The log is bounded by the plan, not by time — a build makes a
//! handful of entries per issue and then stops — which is what makes
//! replay affordable.
//!
//! [`Entry::seq`] is host-wide and monotonic from 0, and is three things
//! at once by design: the deduplication key, the replay cursor, and the
//! id an event stream carries. [`Entry::at`] is unix milliseconds,
//! stamped by the host — the one field the wasm-clean half cannot
//! produce for itself. [`Log`] is where both are assigned, and the only
//! place either is; it alone needs the `native` feature, so the window
//! folds this module's fold rather than a copy of it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::feature::layout::{self, Layout};
use crate::feature::{IssueId, Plan, RunId, State};

/// The event channel entries arrive on, backend to window.
pub const EVENT: &str = "monitor";

/// One thing that happened to a feature build. Every change names its
/// run; the run's life is Reserved, then Started, then Moved as many
/// times as the build moves an issue, then Finished — or Abandoned in
/// place of Started, when the launch failed between reserving and
/// running.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// A run id is held for a launch: recorded before the check card is
    /// raised, so a build is visible from the moment it is asked for.
    Reserved {
        run: RunId,
        feature: IssueId,
        repository: String,
        branch: String,
        base: Option<String>,
    },
    /// The build is running. The plan, what is wrong with its shape —
    /// [`Problem`](crate::feature::Problem)'s own words, because a pane
    /// only ever displays prose — every issue of work with the state it
    /// starts in, and the check in force: `None` means the branch is
    /// unchecked.
    Started {
        run: RunId,
        plan: Plan,
        problems: Vec<String>,
        states: BTreeMap<IssueId, State>,
        check: Option<String>,
    },
    /// One issue moved to `state`.
    Moved {
        run: RunId,
        issue: IssueId,
        state: State,
    },
    /// Nothing is ready and nothing is running: every state is terminal.
    Finished { run: RunId },
    /// The launch failed after reserving, or the build was given up;
    /// `reason` is why, in words.
    Abandoned { run: RunId, reason: String },
}

impl Change {
    /// The run this change belongs to.
    #[must_use]
    pub const fn run(&self) -> RunId {
        match self {
            Self::Reserved { run, .. }
            | Self::Started { run, .. }
            | Self::Moved { run, .. }
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
/// Waiting issue is ready when the plan names it so, given what has
/// merged, and blocked otherwise. `total` is every issue of work.
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
/// what Started handed over, and every issue's state as the Moved
/// entries left it. Obtained only by folding — [`Progress::absorb`] is
/// the one way in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Watch {
    run: RunId,
    feature: IssueId,
    repository: String,
    branch: String,
    base: Option<String>,
    /// `None` until Started: a reservation has no plan yet.
    plan: Option<Plan>,
    problems: Vec<String>,
    states: BTreeMap<IssueId, State>,
    check: Option<String>,
    stage: Stage,
}

impl Watch {
    fn reserved(
        run: RunId,
        feature: IssueId,
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
            plan: None,
            problems: Vec::new(),
            states: BTreeMap::new(),
            check: None,
            stage: Stage::Starting,
        }
    }

    #[must_use]
    pub const fn run(&self) -> RunId {
        self.run
    }

    #[must_use]
    pub const fn feature(&self) -> &IssueId {
        &self.feature
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

    /// The plan the build runs, once Started has said what it is.
    #[must_use]
    pub const fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    /// The feature's title, once Started has handed over the plan that
    /// carries it.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.plan.as_ref().map(Plan::title)
    }

    /// The plan as a picture, every issue drawn where it stands — or
    /// nothing, before there is a plan.
    #[must_use]
    pub fn layout(&self) -> Option<Layout> {
        self.plan
            .as_ref()
            .map(|plan| layout::layout(plan, &self.states))
    }

    /// What is wrong with the plan's shape, in words.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Every issue of work and where it stands.
    #[must_use]
    pub const fn states(&self) -> &BTreeMap<IssueId, State> {
        &self.states
    }

    /// The check in force; `None` is an unchecked branch.
    #[must_use]
    pub fn check(&self) -> Option<&str> {
        self.check.as_deref()
    }

    #[must_use]
    pub const fn stage(&self) -> &Stage {
        &self.stage
    }

    /// Whether an Agent holds a slot right now: some issue is Running.
    /// Merging is not live — no Agent holds a slot during a merge — so
    /// a pane that shows motion for the present shows it only here.
    #[must_use]
    pub fn live(&self) -> bool {
        self.states
            .values()
            .any(|state| matches!(state, State::Running))
    }

    /// How many issues stand where, Waiting split into ready and blocked
    /// by the plan's own judgement over what has merged.
    #[must_use]
    pub fn counts(&self) -> Counts {
        let done: BTreeSet<IssueId> = self
            .states
            .iter()
            .filter(|(_, state)| matches!(state, State::Merged { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        let ready: BTreeSet<&IssueId> = self
            .plan
            .iter()
            .flat_map(|plan| plan.ready(&done))
            .map(|issue| &issue.id)
            .collect();
        let mut counts = Counts {
            total: self.states.len(),
            ..Counts::default()
        };
        for (id, state) in &self.states {
            match state {
                State::Waiting if ready.contains(id) => counts.ready += 1,
                State::Waiting => counts.blocked += 1,
                State::Running => counts.running += 1,
                State::Merging => counts.merging += 1,
                State::Merged { .. } => counts.merged += 1,
                State::Failed { .. } => counts.failed += 1,
                State::Skipped { .. } => counts.skipped += 1,
            }
        }
        counts
    }

    fn apply(&mut self, change: Change) {
        match change {
            Change::Reserved { .. } => {}
            Change::Started {
                plan,
                problems,
                states,
                check,
                ..
            } => {
                self.plan = Some(plan);
                self.problems = problems;
                self.states = states;
                self.check = check;
                self.stage = Stage::Building;
            }
            Change::Moved { issue, state, .. } => {
                self.states.insert(issue, state);
            }
            Change::Finished { .. } => self.stage = Stage::Finished,
            Change::Abandoned { reason, .. } => self.stage = Stage::Abandoned { reason },
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
        match entry.change.clone() {
            Change::Reserved {
                run,
                feature,
                repository,
                branch,
                base,
            } => {
                self.watches
                    .insert(run, Watch::reserved(run, feature, repository, branch, base));
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
pub use log::Log;

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

    impl Log {
        /// An empty log on the system clock.
        #[must_use]
        pub fn new() -> Self {
            Self::with_clock(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| {
                        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
                    })
            })
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
            entries
                .get(usize::try_from(cursor).unwrap_or(usize::MAX)..)
                .map(<[Entry]>::to_vec)
                .unwrap_or_default()
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
            entries
                .get(start..)
                .map(<[Entry]>::to_vec)
                .unwrap_or_default()
        }
    }
}

#[cfg(all(test, feature = "native"))]
pub(crate) use tests::native::folded;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::fixtures::{id, node, plan, two_then_three};

    fn reserved(run: u64) -> Change {
        Change::Reserved {
            run: RunId(run),
            feature: id(1),
            repository: "/r".to_owned(),
            branch: "feature-1".to_owned(),
            base: Some("main".to_owned()),
        }
    }

    fn started(run: u64, plan: Plan) -> Change {
        let states = plan
            .ready(&BTreeSet::new())
            .into_iter()
            .map(|issue| (issue.id.clone(), State::Waiting))
            .collect();
        Change::Started {
            run: RunId(run),
            problems: plan.problems().iter().map(ToString::to_string).collect(),
            plan,
            states,
            check: None,
        }
    }

    fn moved(run: u64, issue: u64, state: State) -> Change {
        Change::Moved {
            run: RunId(run),
            issue: id(issue),
            state,
        }
    }

    fn entry(seq: u64, change: Change) -> Entry {
        Entry {
            seq,
            at: 1_000 + seq,
            change,
        }
    }

    fn merged() -> State {
        State::Merged {
            commit: "abc".to_owned(),
            checked: false,
        }
    }

    #[test]
    fn a_change_serializes_tagged_by_its_word_and_reads_back() {
        let change = moved(1, 2, State::Running);
        let wire = serde_json::to_value(&change).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "change": "moved", "run": 1, "issue": "2", "state": { "state": "running" } })
        );
        let back: Change = serde_json::from_value(wire).unwrap();
        assert_eq!(back, change);

        let entry = entry(3, started(1, two_then_three()));
        let back: Entry = serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn the_fold_walks_a_run_through_its_stages() {
        let mut progress = Progress::default();
        assert!(progress.absorb(&entry(0, reserved(1))));
        let watch = progress.watch(RunId(1)).unwrap();
        assert_eq!(*watch.stage(), Stage::Starting);
        assert_eq!(watch.feature(), &id(1));
        assert_eq!((watch.title(), watch.layout()), (None, None), "no plan yet");
        assert_eq!(watch.branch(), "feature-1");
        assert_eq!(watch.base(), Some("main"));
        assert!(watch.plan().is_none());
        assert_eq!(watch.counts(), Counts::default());

        assert!(progress.absorb(&entry(1, started(1, two_then_three()))));
        let watch = progress.watch(RunId(1)).unwrap();
        assert_eq!(*watch.stage(), Stage::Building);
        assert!(watch.plan().is_some());
        assert_eq!(watch.title(), Some("issue 1"));
        assert_eq!(
            watch.layout().unwrap().nodes.len(),
            watch.states().len(),
            "the picture draws every issue the build holds a state for"
        );
        assert!(!watch.live(), "nothing is running yet");

        assert!(progress.absorb(&entry(2, moved(1, 2, State::Running))));
        assert!(progress.watch(RunId(1)).unwrap().live());
        assert!(progress.absorb(&entry(3, moved(1, 2, State::Merging))));
        assert!(
            !progress.watch(RunId(1)).unwrap().live(),
            "no Agent holds a slot during a merge"
        );
        assert!(progress.absorb(&entry(4, moved(1, 2, merged()))));
        assert!(progress.absorb(&entry(5, Change::Finished { run: RunId(1) })));
        let watch = progress.watch(RunId(1)).unwrap();
        assert_eq!(*watch.stage(), Stage::Finished);
        assert_eq!(watch.states()[&id(2)], merged());
        assert_eq!(progress.applied(), Some(5));
        assert_eq!(progress.watches().count(), 1);
    }

    #[test]
    fn an_abandoned_launch_keeps_its_reason() {
        let mut progress = Progress::default();
        progress.absorb(&entry(0, reserved(1)));
        progress.absorb(&entry(
            1,
            Change::Abandoned {
                run: RunId(1),
                reason: "no plan".to_owned(),
            },
        ));
        assert_eq!(
            *progress.watch(RunId(1)).unwrap().stage(),
            Stage::Abandoned {
                reason: "no plan".to_owned()
            }
        );
    }

    #[test]
    fn an_entry_not_past_the_highest_applied_is_ignored() {
        let mut progress = Progress::default();
        let entries = [
            entry(0, reserved(1)),
            entry(1, started(1, two_then_three())),
            entry(2, moved(1, 2, State::Running)),
        ];
        for entry in &entries {
            assert!(progress.absorb(entry));
        }
        let once = progress.clone();
        for entry in &entries {
            assert!(!progress.absorb(entry), "heard twice: {entry:?}");
        }
        assert_eq!(progress, once);

        // A stale entry with a changed payload is still stale.
        assert!(!progress.absorb(&entry(1, moved(1, 2, merged()))));
        assert_eq!(progress, once);
        // The next one counts.
        assert!(progress.absorb(&entry(3, moved(1, 2, State::Merging))));
    }

    #[test]
    fn a_change_to_a_run_never_reserved_has_nowhere_to_land() {
        let mut progress = Progress::default();
        assert!(progress.absorb(&entry(0, moved(9, 2, State::Running))));
        assert_eq!(progress.applied(), Some(0), "the cursor still moved");
        assert!(progress.watch(RunId(9)).is_none());
    }

    /// 2 waits on nothing and 3 waits on 2: one ready, one blocked, and
    /// the split moves as 2 lands.
    #[test]
    fn counts_split_waiting_into_ready_and_blocked() {
        let plan = plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
            ],
        );
        let states = [(id(2), State::Waiting), (id(3), State::Waiting)].into();
        let mut progress = Progress::default();
        progress.absorb(&entry(0, reserved(1)));
        progress.absorb(&entry(
            1,
            Change::Started {
                run: RunId(1),
                plan,
                problems: Vec::new(),
                states,
                check: None,
            },
        ));
        let counts = progress.watch(RunId(1)).unwrap().counts();
        assert_eq!(
            counts,
            Counts {
                ready: 1,
                blocked: 1,
                total: 2,
                ..Counts::default()
            }
        );

        progress.absorb(&entry(2, moved(1, 2, merged())));
        let counts = progress.watch(RunId(1)).unwrap().counts();
        assert_eq!(
            counts,
            Counts {
                merged: 1,
                ready: 1,
                total: 2,
                ..Counts::default()
            },
            "2 landing released 3"
        );
    }

    #[cfg(feature = "native")]
    pub(crate) mod native {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        use super::*;

        /// The whole log, folded: what a watcher sees after the replay.
        pub(crate) fn folded(log: &Log) -> Progress {
            let mut progress = Progress::default();
            for entry in log.since(0) {
                progress.absorb(&entry);
            }
            progress
        }

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
            assert!(log.since(2).is_empty());
            assert!(log.since(u64::MAX).is_empty());
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
            assert!(log.wait_after(1, Duration::from_millis(20)).is_empty());
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
}
