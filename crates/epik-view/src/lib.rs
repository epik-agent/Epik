//! What the Epik app's windows show: the view models `epik-app` computes
//! and its windows draw.
//!
//! Only types live here, with no dependency but `serde`, so a front end
//! compiled to WebAssembly can deserialize what the app serializes. The
//! functions that build them are in `epik-app`, and in `epik-core` for
//! [`Stream`].

use serde::{Deserialize, Serialize};

/// A piece of a sentence: text, which may link somewhere, and may be code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Left out of the JSON when false, so it defaults to false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
}

/// A sentence of [`Span`]s.
pub type Words = Vec<Span>;

/// How many of a run's issues are merged or closed, running, and not
/// started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub merged: usize,
    pub running: usize,
    pub not_started: usize,
}

/// An issue's state, as its card is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Merged,
    Running,
    NotStarted,
    Failed,
}

/// A run's issues laid out as a dependency graph: a column per level, left
/// to right, so every blocker is left of what it blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub columns: usize,
    pub rows: usize,
    /// Each issue's number, column and row, in the run's order.
    pub places: Vec<(u64, usize, usize)>,
    /// Each blocker and the issue it blocks.
    pub edges: Vec<(u64, u64)>,
}

/// A run's small graph, as a card draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MiniGraph {
    pub layout: Layout,
    /// Each issue's state, in the layout's order.
    pub states: Vec<NodeState>,
}

/// A running issue, in a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NowLine {
    pub number: u64,
    pub words: Words,
    /// "active 2s ago" or "quiet 1m 50s".
    pub life: Option<String>,
    /// When its agent last logged, in milliseconds, so the page flashes the
    /// line when it changes.
    pub last_event: Option<i64>,
}

/// A run, as Home and the popover show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCard {
    /// The run, `owner/name#number`.
    pub run: String,
    pub title: String,
    /// The epic issue, `owner/name#number`, linked.
    pub epic: Span,
    /// "6 issues, 2 merged" or "1 issue".
    pub count: String,
    pub disposition: Disposition,
    pub elapsed: String,
    pub cost: String,
    pub graph: MiniGraph,
    pub progress: Progress,
    /// Each running issue.
    pub now: Vec<NowLine>,
    /// What starts next, while building.
    pub next: Option<Words>,
    /// What is merged, while building.
    pub done: Option<Words>,
    /// Where it stands, in a sentence, when it isn't building.
    pub sentence: Option<Words>,
    /// A second line under it.
    pub note: Option<String>,
    /// For the popover: where it stands, while building.
    pub summary: Option<Words>,
}

/// The main window's Home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Home {
    /// What is happening, in a sentence.
    pub headline: String,
    pub building: Vec<RunCard>,
    pub waiting: Vec<RunCard>,
    pub attention: Vec<RunCard>,
}

/// A run, as the main window's sidebar lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarRun {
    /// The run, `owner/name#number`.
    pub run: String,
    pub title: String,
    pub disposition: Disposition,
}

/// The main window's sidebar, beside every page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidebar {
    /// The monitored runs: those needing attention, then building, then
    /// waiting for review.
    pub runs: Vec<SidebarRun>,
    /// Whether any monitored repository is no longer on GitHub, which
    /// marks Settings.
    pub missing_repository: bool,
    /// "Epik running · up 4h 12m".
    pub status: String,
}

/// The tray's popover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Popover {
    /// "Two runs building".
    pub headline: String,
    pub building: Vec<RunCard>,
    /// A sentence for each run waiting for review or needing attention.
    pub waiting: Vec<(Disposition, String, Words)>,
}

/// An issue's card on the run page, and what its detail panel says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCard {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: NodeState,
    pub column: usize,
    pub row: usize,
    /// "3m 05s · $0.92", once known.
    pub figures: Option<String>,
    /// The card's line: "Merged in PR #20", "cargo test · last: 23
    /// passed, 1 failed", "Starts when #16 and #17 are merged".
    pub line: Words,
    pub life: Option<String>,
    pub last_event: Option<i64>,
    /// The detail panel's sentence about its state.
    pub sentence: Words,
    /// "2m 40s · $0.48 so far".
    pub worked: Option<String>,
    pub depends_on: Words,
    pub unblocks: Words,
    pub pull_request: Words,
    /// What its agent has done, in words, oldest first.
    pub recent: Vec<String>,
    /// Its agent's full log, if it has one.
    pub log: Option<String>,
}

/// The run page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPage {
    pub run: String,
    pub title: String,
    pub epic: Span,
    /// "6 issues · merging into epic/13-exponentiation".
    pub about: Words,
    /// "Second build running. 2 of 6 issues merged, 2 agents working."
    pub sentence: String,
    pub disposition: Disposition,
    pub elapsed: String,
    pub cost: String,
    /// Another run building, to link to: its key and title.
    pub also_building: Vec<(String, String)>,
    /// Each build in a sentence, the current one first.
    pub builds: Vec<String>,
    pub columns: usize,
    pub rows: usize,
    pub issues: Vec<IssueCard>,
    pub edges: Vec<(u64, u64)>,
}

/// A monitored repository, as Settings lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRow {
    pub repo: String,
    pub url: String,
    /// "2 runs · added Oct 20" or "No runs · added by you, Oct 28".
    pub about: String,
    /// The runs that leave the window if it is no longer monitored: each
    /// one's key and title.
    pub runs: Vec<(String, String)>,
    /// Whether GitHub no longer finds it.
    pub missing: bool,
}

/// Settings: the monitored repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub repositories: Vec<RepositoryRow>,
}

/// One item as the checklist shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub state: ItemState,
    pub detail: String,
    /// The button's label, for an item that isn't done.
    pub button: Option<String>,
}

/// The checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checklist {
    pub rows: Vec<Row>,
    /// Whether every item is done.
    pub complete: bool,
}

/// What a pressed button has printed, and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub lines: Vec<String>,
    pub running: bool,
    pub error: Option<String>,
}

/// Where a run stands, as the window sorts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// A build is running.
    Building,
    /// Every issue merged, and its pull request into the default branch
    /// waits for the user.
    WaitingForReview,
    /// Its latest build failed or was interrupted, or its pull request was
    /// closed without merging.
    NeedsAttention,
}

/// Where a setup item stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Done,
    Missing,
    Outdated,
}

/// What the confirmation to uninstall Epik needs to say.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uninstall {
    /// How many runs' history uninstalling deletes.
    pub runs: usize,
    /// Whether a build is running, which keeps Epik from being uninstalled.
    pub building: bool,
}

/// How one step of uninstalling went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallStep {
    /// What it removed: "Run history".
    pub title: String,
    /// Why it couldn't, if it couldn't.
    pub error: Option<String>,
}

/// An agent's log as people read it: what it said and did, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stream {
    pub entries: Vec<StreamEntry>,
    /// Whether its last tool call hasn't completed, so the agent is still
    /// at work.
    pub working: bool,
}

/// One thing an agent said or did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEntry {
    /// When it began, in milliseconds since the Unix epoch. None in a log
    /// written before Epik recorded times.
    pub at: Option<i64>,
    pub what: Entry,
}

/// What an agent said or did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entry {
    /// The agent's own words.
    Narration { text: String },
    /// A command line and what it printed.
    Command { command: String, output: Excerpt },
    /// A file read, and which of its lines, if not all of them.
    Read {
        path: String,
        lines: Option<LineRange>,
    },
    /// A search: its pattern, where it looked, how many matches it found,
    /// and where they are, as `file:line` or a file.
    Search {
        pattern: String,
        within: Option<String>,
        matches: usize,
        hits: Excerpt,
    },
    /// A file changed: the lines it removed and added.
    Edit {
        path: String,
        changes: Excerpt<Change>,
    },
    /// A file written whole.
    NewFile { path: String, content: Excerpt },
    /// Any other tool call, in words: "Updating its plan".
    Other { text: String },
}

/// Lines of a file, counted from 1. `to` is None when the range runs to
/// the end of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub from: u64,
    pub to: Option<u64>,
}

/// A line an edit removed or added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Removed(String),
    Added(String),
}

/// Lines cut down to fit: all of them if there are few, else the first
/// and last, with "… N more lines" between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Excerpt<T = String> {
    pub head: Vec<T>,
    /// How many lines are left out between `head` and `tail`.
    pub more: usize,
    pub tail: Vec<T>,
}

impl<T> Default for Excerpt<T> {
    fn default() -> Self {
        Self {
            head: Vec::new(),
            more: 0,
            tail: Vec::new(),
        }
    }
}
