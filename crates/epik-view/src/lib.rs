//! What the Epik app's windows show: the view models `epik-app` computes
//! and its windows draw.
//!
//! Only types live here, with no dependency but `serde`, so a front end
//! compiled to WebAssembly can deserialize what the app serializes. The
//! functions that build them are in `epik-app`, and in `epik-core` for
//! [`Stream`].

use std::path::PathBuf;

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

/// A run, as Home shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCard {
    /// The run, `owner/name#number`, after `<host>:` on a remote host.
    pub run: String,
    pub title: String,
    /// Where it builds, when there is more than one host: "Building on
    /// devbox", or for a host that can't be reached, when it was last heard
    /// from.
    pub place: Option<String>,
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
    /// Where it stands, while building.
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
    /// The run, `owner/name#number`, after `<host>:` on a remote host.
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

/// A building run, as the tray menu lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayRun {
    /// The run, `owner/name#number`, after `<host>:` on a remote host.
    pub run: String,
    pub title: String,
    /// Where it builds, when there is more than one host: "Building on
    /// devbox", or for a host that can't be reached, when it was last heard
    /// from.
    pub place: Option<String>,
    /// How many of its issues are merged, of all of them: "3/7".
    pub count: String,
    /// Each issue's state, in the run's order: a segment each of its
    /// progress bar.
    pub segments: Vec<NodeState>,
    /// "building #443 · 2m 00s · epik-agent/Epik#441".
    pub line: String,
}

/// The tray menu: whether Epik is running, and each building run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Popover {
    /// "3 runs · 4 agents · up 4h 12m".
    pub about: String,
    /// What Restart asks before it restarts: "Interrupt two builds?".
    pub restart: String,
    /// The building runs.
    pub runs: Vec<TrayRun>,
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
    /// Its latest agent session's log, the one its stream shows unless
    /// another session is chosen.
    pub log: Option<String>,
    /// Its agent sessions, oldest first.
    pub sessions: Vec<SessionRow>,
}

/// An agent session, as an issue's panel and a run's header list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRow {
    /// "Coding" or "Review".
    pub role: String,
    /// Which session of its role this is, from 1.
    pub round: u32,
    /// "Claude Code".
    pub agent: String,
    /// "opus", or "default model".
    pub model: String,
    /// "3m 05s", or "2m 00s so far" while it runs.
    pub time: String,
    /// "$0.92" or "not reported", once it has finished.
    pub cost: Option<String>,
    /// A review's verdict: "approved", "changes requested" or "no verdict".
    pub verdict: Option<String>,
    /// Beside the run's last review, the threads still open: "2 threads
    /// open", linked to the pull request's Files view.
    pub threads: Option<Span>,
    /// Whether it is running.
    pub running: bool,
    /// Its log's path, for its stream.
    pub log: Option<String>,
}

/// The run page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPage {
    /// The run, `owner/name#number`, after `<host>:` on a remote host.
    pub run: String,
    pub title: String,
    /// Where it builds, when there is more than one host: "Building on
    /// devbox", or for a host that can't be reached, when it was last heard
    /// from.
    pub place: Option<String>,
    pub epic: Span,
    /// "6 issues · merging into epic/13-exponentiation".
    pub about: Words,
    /// "Second build running. 2 of 6 issues merged, 2 agents working."
    pub sentence: String,
    pub disposition: Disposition,
    pub elapsed: String,
    pub cost: String,
    /// Another run building, to link to: its name and title.
    pub also_building: Vec<(String, String)>,
    /// Each build in a sentence, the current one first.
    pub builds: Vec<String>,
    pub columns: usize,
    pub rows: usize,
    pub issues: Vec<IssueCard>,
    pub edges: Vec<(u64, u64)>,
    /// The run's own sessions, its reviews and the fixes they asked for,
    /// oldest first.
    pub sessions: Vec<SessionRow>,
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

/// Settings: the monitored repositories, and the hosts builds can run on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub repositories: Vec<RepositoryRow>,
    /// This computer first, then each remote host in the order added.
    pub hosts: Vec<HostRow>,
}

/// A host builds can run on, as Settings lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRow {
    /// Its SSH destination, or `None` for this computer.
    pub destination: Option<String>,
    /// "This computer", or the destination.
    pub name: String,
    pub status: HostStatus,
    /// "Epik 0.4.0 · setup complete · 2 runs".
    pub about: String,
    /// Why it can't build: what `ssh` said, or which Epik to install there.
    pub why: Option<String>,
    /// Each setup item a build needs that it lacks, with what to run there.
    pub missing: Vec<SetupFix>,
    /// Whether builds go to it unless told otherwise.
    pub default: bool,
    /// The runs that leave the window if it is removed: each one's key and
    /// title.
    pub runs: Vec<(String, String)>,
}

/// Whether a host can build, as its row's mark shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostStatus {
    /// It answered, and nothing it is known to need is missing.
    Ready,
    /// Not heard from since it was added.
    Unchecked,
    /// It answered, and lacks something a build needs.
    Incomplete,
    /// It couldn't be reached.
    Unreachable,
    /// Its app is another minor version than this one's.
    Mismatch,
}

/// A setup item a host lacks, and the commands to run there for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupFix {
    /// "GitHub sign-in".
    pub title: String,
    /// "not signed in".
    pub detail: String,
    /// Each command to run on the host, in order.
    pub commands: Vec<String>,
}

/// The agent and model each role runs with, as `~/.epik/settings.json`
/// keeps them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSettings {
    /// Implementing one issue, up to a pull request with passing checks.
    pub coding: Role,
    /// Reading a build's pull requests before the user does, or `None`
    /// when review is off.
    pub review: Option<Role>,
}

impl Default for AgentSettings {
    /// Claude Code codes with its default model, and nothing reviews.
    fn default() -> Self {
        Self {
            coding: Role {
                agent: "claude".to_owned(),
                model: None,
            },
            review: None,
        }
    }
}

/// The agent a role runs, and the model it asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    /// The agent's id: `claude`.
    pub agent: String,
    /// The model, or `None` for the agent's default.
    pub model: Option<String>,
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
    /// A build is running, and is reviewing the run's pull request.
    Reviewing,
    /// Every issue merged, and its pull request into the default branch
    /// waits for the user.
    WaitingForReview,
    /// Its latest build failed or was interrupted, or its pull request was
    /// closed without merging.
    NeedsAttention,
}

impl Disposition {
    /// Whether a build of the run is running, reviewing or not.
    pub fn is_building(self) -> bool {
        matches!(self, Disposition::Building | Disposition::Reviewing)
    }
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
    /// How the user removes the app once Epik's files are gone.
    pub removal: Removal,
}

/// How the user removes the app, the way their platform removes any app.
/// The app doesn't remove itself: each platform's installer owns it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Removal {
    /// The bundle at this path, e.g. /Applications/Epik.app.
    MacApp(PathBuf),
    /// Installed with the .msi.
    WindowsApp,
    /// The AppImage at this path, from $APPIMAGE.
    AppImage(PathBuf),
    /// Installed with the .deb.
    #[default]
    Deb,
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
