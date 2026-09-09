//! The monitor's view: a pure fold beside the library's own.
//!
//! [`Progress`] folds the feed's entries into every build the host has
//! heard of; [`View`] holds that fold and what the page adds to it —
//! which tab is open, which issue each feature has selected, which
//! finished tabs have been closed, and whether the feed is still
//! speaking. Every rule about what the user sees is a method here,
//! tested on the host with plain `cargo test`; the components in
//! [`pane`](crate::pane) draw the answers and nothing else.
//!
//! Entries reach the page through the [`Feed`] seam. The order is
//! listen, then replay: [`attach`] hears live entries from the moment the
//! listener is up and holds them until the replay has been folded, then
//! folds what it held — [`Progress::absorb`] drops by `seq` whatever the
//! replay already covered — so an entry recorded between the two is
//! applied exactly once. (`Chat` does history-then-listen without that
//! protection; this fold does not copy it.) A blink is a claim about the
//! present, so the feed's health is view state: while it is not
//! speaking, nothing pulses.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use epik::feature::layout::{Form, Layout, Node};
use epik::feature::{IssueId, RunId, State};
use epik::monitor::{Counts, Entry, Progress, Stage, Watch};

/// Where entries come from: a channel that delivers each as it lands,
/// and a replay of everything so far. The window's is Tauri's event
/// channel and the `monitor_log` command; a browser's is an event
/// stream over HTTP.
pub trait Feed {
    /// Hears every entry from now on, for the life of the page, then
    /// says whether the listener is up — the Err is why it is not.
    fn listen(
        &self,
        hear: impl Fn(Entry) + 'static,
        attached: impl FnOnce(Result<(), String>) + 'static,
    );

    /// Every entry recorded so far.
    fn replay(&self, deliver: impl FnOnce(Result<Vec<Entry>, String>) + 'static);
}

/// What the feed does to the view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Step {
    /// The listener delivered an entry.
    Heard(Entry),
    /// The replay arrived, or the channel refused it.
    Replayed(Result<Vec<Entry>, String>),
    /// The feed stopped, and why.
    Lost(String),
}

/// Listen, then replay. `apply` takes each [`Step`] to the view; the
/// replay is requested only once the listener is up, so nothing
/// recorded between the two can be missed.
pub fn attach<F: Feed + 'static>(feed: F, apply: impl Fn(Step) + Clone + 'static) {
    let feed = Rc::new(feed);
    let hear = {
        let apply = apply.clone();
        move |entry| apply(Step::Heard(entry))
    };
    let attached = {
        let feed = Rc::clone(&feed);
        move |outcome: Result<(), String>| {
            if let Err(reason) = outcome {
                apply(Step::Lost(reason));
            }
            feed.replay(move |replay| apply(Step::Replayed(replay)));
        }
    };
    feed.listen(hear, attached);
}

/// The feed's standing: attaching — holding what the listener hears
/// until the replay has been folded — attached, or lost.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Link {
    Attaching(Vec<Entry>),
    Attached,
    Lost(String),
}

/// Which tab is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tab {
    Chat,
    Feature(RunId),
}

/// The colour of a tab's dot: error when anything failed, open while
/// work is in flight, closed when the build finished clean, muted
/// otherwise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dot {
    Error,
    Open,
    Closed,
    Muted,
}

/// One tab in the strip, decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabSpec {
    pub run: RunId,
    /// `#158 the feature's title`, or the id alone before the plan.
    pub label: String,
    pub dot: Dot,
    /// Whether the dot pulses: an Agent is working, and the feed is
    /// speaking.
    pub pulse: bool,
    /// `3/7`: merged over total, once there is a plan.
    pub count: Option<String>,
    /// A finished or abandoned build's tab can be closed.
    pub closable: bool,
    pub active: bool,
}

/// The selected issue, as the detail column says it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Detail {
    pub heading: String,
    /// Where it stands, as a sentence.
    pub sentence: String,
    pub form: Form,
    /// The issues it blocks, as `#3, #5`, or `nothing`.
    pub blocks: String,
}

/// One feature's pane, decided.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub run: RunId,
    pub heading: String,
    /// `repository · branch`.
    pub origin: String,
    /// The counts as words: `3 merged · 2 running · 2 blocked`.
    pub counts: String,
    /// Why the picture may be behind: the feed is not speaking.
    pub disconnected: Option<String>,
    /// What the build said of itself: an abandoned launch's reason,
    /// the plan's problems.
    pub notices: Vec<String>,
    /// Whether Running nodes pulse: the feed is speaking.
    pub pulse: bool,
    /// The picture, once there is a plan.
    pub layout: Option<Layout>,
    pub selected: Option<IssueId>,
    pub detail: Option<Detail>,
}

/// The whole view: the library's fold plus the page's own state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct View {
    progress: Progress,
    link: Link,
    tab: Tab,
    selected: BTreeMap<RunId, IssueId>,
    closed: BTreeSet<RunId>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            progress: Progress::default(),
            link: Link::Attaching(Vec::new()),
            tab: Tab::Chat,
            selected: BTreeMap::new(),
            closed: BTreeSet::new(),
        }
    }
}

impl View {
    /// Folds one step of the feed in, and says how many entries counted.
    /// An entry heard while the replay is still on its way is held, not
    /// folded; the replay is folded first and the held entries after
    /// it, each counting once or not at all by `seq`.
    pub fn step(&mut self, step: Step) -> usize {
        match step {
            Step::Heard(entry) => match &mut self.link {
                Link::Attaching(held) => {
                    held.push(entry);
                    0
                }
                Link::Attached | Link::Lost(_) => usize::from(self.progress.absorb(&entry)),
            },
            Step::Replayed(Ok(entries)) => {
                let held = match std::mem::replace(&mut self.link, Link::Attached) {
                    Link::Attaching(held) => held,
                    Link::Attached => Vec::new(),
                    Link::Lost(reason) => {
                        self.link = Link::Lost(reason);
                        Vec::new()
                    }
                };
                entries
                    .iter()
                    .chain(&held)
                    .filter(|entry| self.progress.absorb(entry))
                    .count()
            }
            Step::Replayed(Err(reason)) | Step::Lost(reason) => {
                self.link = Link::Lost(reason);
                0
            }
        }
    }

    /// Whether the feed is speaking: the only state in which anything
    /// pulses.
    #[must_use]
    pub fn connected(&self) -> bool {
        self.link == Link::Attached
    }

    /// Why the picture may be behind, when the feed is not speaking.
    #[must_use]
    pub fn disconnected(&self) -> Option<String> {
        match &self.link {
            Link::Attached => None,
            Link::Attaching(_) => Some("connecting to the build feed".to_owned()),
            Link::Lost(reason) => Some(format!("the build feed stopped: {reason}")),
        }
    }

    #[must_use]
    pub const fn tab(&self) -> Tab {
        self.tab
    }

    pub fn open(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// Closes a finished build's tab; a build still going stays. Closing
    /// the open tab returns to the chat.
    pub fn close(&mut self, run: RunId) {
        let closable = self
            .progress
            .watch(run)
            .is_some_and(|watch| closable(watch.stage()));
        if !closable {
            return;
        }
        self.closed.insert(run);
        if self.tab == Tab::Feature(run) {
            self.tab = Tab::Chat;
        }
    }

    /// Selects `issue` in `run`'s pane, remembered across tab switches.
    pub fn select(&mut self, run: RunId, issue: IssueId) {
        self.selected.insert(run, issue);
    }

    /// Every build's tab, oldest first, the closed ones gone.
    #[must_use]
    pub fn tabs(&self) -> Vec<TabSpec> {
        self.progress
            .watches()
            .filter(|watch| !self.closed.contains(&watch.run()))
            .map(|watch| {
                let counts = watch.counts();
                TabSpec {
                    run: watch.run(),
                    label: label(watch),
                    dot: dot(watch, counts),
                    pulse: self.connected() && watch.live(),
                    count: watch
                        .plan()
                        .map(|_| format!("{}/{}", counts.merged, counts.total)),
                    closable: closable(watch.stage()),
                    active: self.tab == Tab::Feature(watch.run()),
                }
            })
            .collect()
    }

    /// The open feature's pane, or nothing on the chat tab.
    #[must_use]
    pub fn pane(&self) -> Option<Pane> {
        let Tab::Feature(run) = self.tab else {
            return None;
        };
        let watch = self.progress.watch(run)?;
        let layout = watch.layout();
        let selected = self.selected.get(&run).cloned();
        let detail = selected
            .as_ref()
            .and_then(|id| layout.as_ref()?.node(id))
            .map(detail);
        let mut notices = Vec::new();
        if let Stage::Abandoned { reason } = watch.stage() {
            notices.push(format!("abandoned: {reason}"));
        }
        notices.extend(watch.problems().iter().cloned());
        Some(Pane {
            run,
            heading: label(watch),
            origin: format!("{} · {}", watch.repository(), watch.branch()),
            counts: words(watch.stage(), watch.counts()),
            disconnected: self.disconnected(),
            notices,
            pulse: self.connected(),
            layout,
            selected,
            detail,
        })
    }
}

const fn closable(stage: &Stage) -> bool {
    matches!(stage, Stage::Finished | Stage::Abandoned { .. })
}

/// `#158 the feature's title`, or the id alone before there is a plan.
fn label(watch: &Watch) -> String {
    match watch.title() {
        Some(title) => format!("#{} {title}", watch.feature()),
        None => format!("#{}", watch.feature()),
    }
}

/// Error when anything failed; open while work is in flight; closed when
/// finished with nothing failed or skipped; muted otherwise — starting,
/// building with nothing in flight, abandoned.
const fn dot(watch: &Watch, counts: Counts) -> Dot {
    if counts.failed > 0 {
        Dot::Error
    } else if counts.running > 0 || counts.merging > 0 {
        Dot::Open
    } else if matches!(watch.stage(), Stage::Finished) && counts.skipped == 0 {
        Dot::Closed
    } else {
        Dot::Muted
    }
}

/// The counts as words, the zeros left unsaid.
fn words(stage: &Stage, counts: Counts) -> String {
    let parts: Vec<String> = [
        (counts.merged, "merged"),
        (counts.running, "running"),
        (counts.merging, "merging"),
        (counts.failed, "failed"),
        (counts.skipped, "skipped"),
        (counts.ready, "ready"),
        (counts.blocked, "blocked"),
    ]
    .into_iter()
    .filter(|&(count, _)| count > 0)
    .map(|(count, word)| format!("{count} {word}"))
    .collect();
    if parts.is_empty() {
        match stage {
            Stage::Starting => "starting".to_owned(),
            _ => "no work".to_owned(),
        }
    } else {
        parts.join(" · ")
    }
}

fn names(ids: &[IssueId]) -> String {
    if ids.is_empty() {
        return "nothing".to_owned();
    }
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where a node stands, as a sentence.
#[must_use]
pub fn sentence(node: &Node) -> String {
    match (&node.state, node.form) {
        (State::Waiting, Form::Ready) => "waiting — ready, out of slots".to_owned(),
        (State::Waiting, _) if node.blocked_by.is_empty() => "waiting — blocked".to_owned(),
        (State::Waiting, _) => format!("waiting — blocked by {}", names(&node.blocked_by)),
        (State::Running, _) => "running".to_owned(),
        (State::Merging, _) => "merging".to_owned(),
        (State::Merged { commit, checked }, _) => format!(
            "merged at {} ({})",
            commit.chars().take(7).collect::<String>(),
            if *checked { "checked" } else { "unchecked" }
        ),
        (State::Failed { report }, _) => format!("failed: {report}"),
        (State::Skipped { reason }, _) => format!("skipped: {reason}"),
    }
}

fn detail(node: &Node) -> Detail {
    Detail {
        heading: format!("#{} {}", node.id, node.title),
        sentence: sentence(node),
        form: node.form,
        blocks: names(&node.blocks),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use epik::feature::Plan;
    use epik::monitor::Change;

    use super::*;

    /// The wire form of feature 1 holding 2 and 3, with 3 waiting on 2
    /// — the library's own `two_then_three`, as a window would read it.
    fn two_then_three() -> Plan {
        serde_json::from_value(serde_json::json!({
            "tree": {
                "value": { "id": "1", "title": "the feature", "closed": false },
                "children": [
                    { "value": { "id": "2", "title": "first", "closed": false }, "children": [] },
                    { "value": { "id": "3", "title": "second", "closed": false }, "children": [] }
                ]
            },
            "blocking": [{ "issue": "3", "blocker": "2" }],
            "outside": []
        }))
        .unwrap()
    }

    fn id(number: u64) -> IssueId {
        IssueId::from(number)
    }

    fn entry(seq: u64, change: Change) -> Entry {
        Entry {
            seq,
            at: 1_000 + seq,
            change,
        }
    }

    fn reserved(run: u64) -> Change {
        Change::Reserved {
            run: RunId(run),
            feature: id(1),
            repository: "/r".to_owned(),
            branch: "feature-1".to_owned(),
            base: None,
        }
    }

    fn started(run: u64) -> Change {
        Change::Started {
            run: RunId(run),
            plan: two_then_three(),
            problems: Vec::new(),
            states: [(id(2), State::Waiting), (id(3), State::Waiting)].into(),
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

    fn merged() -> State {
        State::Merged {
            commit: "abc1234def".to_owned(),
            checked: true,
        }
    }

    /// Reserved, Started, 2 Running: the entries a build makes up to
    /// the first Agent.
    fn opening() -> Vec<Entry> {
        vec![
            entry(0, reserved(1)),
            entry(1, started(1)),
            entry(2, moved(1, 2, State::Running)),
        ]
    }

    fn attached(entries: &[Entry]) -> View {
        let mut view = View::default();
        view.step(Step::Replayed(Ok(entries.to_vec())));
        view
    }

    /// A closure the scripted feed was handed, waiting to be fired.
    type Held<F> = RefCell<Option<Box<F>>>;

    /// A feed the test drives by hand: it keeps the closures it is
    /// handed, and the test fires them in the order it wants to try.
    #[derive(Default)]
    struct Scripted {
        hear: Held<dyn Fn(Entry)>,
        attached: Held<dyn FnOnce(Result<(), String>)>,
        deliver: Held<dyn FnOnce(Result<Vec<Entry>, String>)>,
    }

    impl Feed for Rc<Scripted> {
        fn listen(
            &self,
            hear: impl Fn(Entry) + 'static,
            attached: impl FnOnce(Result<(), String>) + 'static,
        ) {
            *self.hear.borrow_mut() = Some(Box::new(hear));
            *self.attached.borrow_mut() = Some(Box::new(attached));
        }

        fn replay(&self, deliver: impl FnOnce(Result<Vec<Entry>, String>) + 'static) {
            *self.deliver.borrow_mut() = Some(Box::new(deliver));
        }
    }

    /// The listener is up; entry 2 is recorded and delivered live before
    /// the replay — carrying 0, 1 and 2 — arrives. Every entry is applied
    /// exactly once, and the view is the one a straight fold gives.
    #[test]
    fn an_entry_heard_before_the_replay_is_applied_exactly_once() {
        let feed = Rc::new(Scripted::default());
        let view = Rc::new(RefCell::new(View::default()));
        let counted = Rc::new(RefCell::new(Vec::new()));
        attach(Rc::clone(&feed), {
            let view = Rc::clone(&view);
            let counted = Rc::clone(&counted);
            move |step| {
                let applied = view.borrow_mut().step(step);
                counted.borrow_mut().push(applied);
            }
        });
        assert!(
            feed.deliver.borrow().is_none(),
            "the replay waits for the listener"
        );
        feed.attached.borrow_mut().take().unwrap()(Ok(()));
        assert!(
            feed.deliver.borrow().is_some(),
            "and is asked for once it is up"
        );

        let live = entry(2, moved(1, 2, State::Running));
        feed.hear.borrow().as_ref().unwrap()(live);
        assert_eq!(*counted.borrow(), [0], "held, not folded");
        assert!(!view.borrow().connected());

        feed.deliver.borrow_mut().take().unwrap()(Ok(opening()));
        assert_eq!(
            *counted.borrow(),
            [0, 3],
            "0, 1 and 2 once each; the held 2 not again"
        );
        assert!(view.borrow().connected());
        assert_eq!(*view.borrow(), attached(&opening()));

        feed.hear.borrow().as_ref().unwrap()(entry(3, moved(1, 2, State::Merging)));
        assert_eq!(
            *counted.borrow(),
            [0, 3, 1],
            "live entries fold straight in now"
        );
        feed.hear.borrow().as_ref().unwrap()(entry(3, moved(1, 2, State::Merging)));
        assert_eq!(
            *counted.borrow(),
            [0, 3, 1, 0],
            "and a repeat counts for nothing"
        );
    }

    #[test]
    fn a_listener_that_fails_to_attach_still_gets_the_replay_but_is_not_connected() {
        let feed = Rc::new(Scripted::default());
        let view = Rc::new(RefCell::new(View::default()));
        attach(Rc::clone(&feed), {
            let view = Rc::clone(&view);
            move |step| {
                view.borrow_mut().step(step);
            }
        });
        feed.attached.borrow_mut().take().unwrap()(Err("no channel".to_owned()));
        feed.deliver.borrow_mut().take().unwrap()(Ok(opening()));
        let view = view.borrow();
        assert!(!view.connected());
        assert_eq!(
            view.disconnected().as_deref(),
            Some("the build feed stopped: no channel")
        );
        assert_eq!(view.tabs().len(), 1, "the picture is still shown");
        assert!(!view.tabs()[0].pulse, "but nothing pulses");
    }

    #[test]
    fn a_reservation_opens_a_tab_at_once_in_the_starting_stage() {
        let view = attached(&[entry(0, reserved(1))]);
        assert_eq!(
            view.tabs(),
            [TabSpec {
                run: RunId(1),
                label: "#1".to_owned(),
                dot: Dot::Muted,
                pulse: false,
                count: None,
                closable: false,
                active: false,
            }]
        );
        let mut view = view;
        view.open(Tab::Feature(RunId(1)));
        let pane = view.pane().unwrap();
        assert_eq!(pane.counts, "starting");
        assert!(pane.layout.is_none());
        assert_eq!(pane.origin, "/r · feature-1");
    }

    #[test]
    fn the_tab_follows_the_build() {
        let mut view = attached(&opening());
        let tab = &view.tabs()[0];
        assert_eq!(tab.label, "#1 the feature");
        assert_eq!((tab.dot, tab.pulse), (Dot::Open, true));
        assert_eq!(tab.count.as_deref(), Some("0/2"));
        assert!(!tab.closable);

        view.step(Step::Heard(entry(3, moved(1, 2, State::Merging))));
        let tab = &view.tabs()[0];
        assert_eq!((tab.dot, tab.pulse), (Dot::Open, false), "merging is still");

        view.step(Step::Heard(entry(4, moved(1, 2, merged()))));
        let tab = &view.tabs()[0];
        assert_eq!(
            (tab.dot, tab.pulse),
            (Dot::Muted, false),
            "nothing in flight"
        );
        assert_eq!(tab.count.as_deref(), Some("1/2"));

        view.step(Step::Heard(entry(5, moved(1, 3, merged()))));
        view.step(Step::Heard(entry(6, Change::Finished { run: RunId(1) })));
        let tab = &view.tabs()[0];
        assert_eq!((tab.dot, tab.closable), (Dot::Closed, true));
    }

    #[test]
    fn a_failure_colours_the_dot_whatever_else_is_happening() {
        let mut view = attached(&opening());
        view.step(Step::Heard(entry(
            3,
            moved(
                1,
                3,
                State::Failed {
                    report: "red".to_owned(),
                },
            ),
        )));
        let tab = &view.tabs()[0];
        assert_eq!(
            (tab.dot, tab.pulse),
            (Dot::Error, true),
            "2 is still running"
        );
    }

    #[test]
    fn a_build_with_no_running_agent_pulses_nowhere() {
        let mut view = attached(&opening());
        view.step(Step::Heard(entry(3, moved(1, 2, State::Merging))));
        view.open(Tab::Feature(RunId(1)));
        assert!(!view.tabs()[0].pulse);
        let pane = view.pane().unwrap();
        assert!(pane.pulse, "the feed is speaking, so a Running node would");
        assert!(
            pane.layout
                .unwrap()
                .nodes
                .iter()
                .all(|node| node.form != Form::Running),
            "but none is"
        );
    }

    #[test]
    fn losing_the_feed_clears_every_pulse_and_says_so() {
        let mut view = attached(&opening());
        view.open(Tab::Feature(RunId(1)));
        assert!(view.tabs()[0].pulse);
        assert!(view.pane().unwrap().disconnected.is_none());

        view.step(Step::Lost("the wire went quiet".to_owned()));
        assert!(!view.tabs()[0].pulse);
        let pane = view.pane().unwrap();
        assert!(!pane.pulse);
        assert_eq!(
            pane.disconnected.as_deref(),
            Some("the build feed stopped: the wire went quiet")
        );
        assert_eq!(pane.counts, "1 running · 1 blocked", "the picture stays");
    }

    #[test]
    fn selection_survives_a_tab_switch() {
        let mut view = attached(&opening());
        view.open(Tab::Feature(RunId(1)));
        assert!(view.pane().unwrap().detail.is_none());
        view.select(RunId(1), id(3));
        view.open(Tab::Chat);
        assert!(view.pane().is_none());
        view.open(Tab::Feature(RunId(1)));
        let pane = view.pane().unwrap();
        assert_eq!(pane.selected, Some(id(3)));
        assert_eq!(
            pane.detail,
            Some(Detail {
                heading: "#3 second".to_owned(),
                sentence: "waiting — blocked by #2".to_owned(),
                form: Form::Blocked,
                blocks: "nothing".to_owned(),
            })
        );
        assert_eq!(pane.counts, "1 running · 1 blocked");
    }

    #[test]
    fn a_finished_tab_closes_and_a_running_one_does_not() {
        let mut view = attached(&opening());
        view.open(Tab::Feature(RunId(1)));
        view.close(RunId(1));
        assert_eq!(view.tabs().len(), 1, "still building");
        assert_eq!(view.tab(), Tab::Feature(RunId(1)));

        view.step(Step::Heard(entry(3, moved(1, 2, merged()))));
        view.step(Step::Heard(entry(4, moved(1, 3, merged()))));
        view.step(Step::Heard(entry(5, Change::Finished { run: RunId(1) })));
        view.close(RunId(1));
        assert!(view.tabs().is_empty());
        assert_eq!(
            view.tab(),
            Tab::Chat,
            "the open tab closing returns to the chat"
        );
    }

    #[test]
    fn an_abandoned_run_shows_its_reason_and_can_be_closed() {
        let mut view = attached(&[
            entry(0, reserved(1)),
            entry(
                1,
                Change::Abandoned {
                    run: RunId(1),
                    reason: "declined".to_owned(),
                },
            ),
        ]);
        let tab = &view.tabs()[0];
        assert_eq!((tab.dot, tab.closable), (Dot::Muted, true));
        view.open(Tab::Feature(RunId(1)));
        assert_eq!(view.pane().unwrap().notices, ["abandoned: declined"]);
        view.close(RunId(1));
        assert!(view.tabs().is_empty());
    }

    #[test]
    fn the_sentences() {
        let mut view = attached(&opening());
        view.open(Tab::Feature(RunId(1)));
        let says = |view: &mut View, issue: u64| {
            view.select(RunId(1), id(issue));
            view.pane().unwrap().detail.unwrap().sentence
        };
        assert_eq!(says(&mut view, 2), "running");
        view.step(Step::Heard(entry(3, moved(1, 2, State::Merging))));
        assert_eq!(says(&mut view, 2), "merging");
        view.step(Step::Heard(entry(4, moved(1, 2, merged()))));
        assert_eq!(says(&mut view, 2), "merged at abc1234 (checked)");
        assert_eq!(says(&mut view, 3), "waiting — ready, out of slots");
        view.step(Step::Heard(entry(
            5,
            moved(
                1,
                3,
                State::Failed {
                    report: "the check went red".to_owned(),
                },
            ),
        )));
        assert_eq!(says(&mut view, 3), "failed: the check went red");
        view.step(Step::Heard(entry(
            6,
            moved(
                1,
                3,
                State::Skipped {
                    reason: "#2 failed".to_owned(),
                },
            ),
        )));
        assert_eq!(says(&mut view, 3), "skipped: #2 failed");
        assert_eq!(view.pane().unwrap().counts, "1 merged · 1 skipped");
    }
}
