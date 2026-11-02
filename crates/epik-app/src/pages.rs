//! What the app's window and tray menu show, as pure functions of the runs.
//!
//! The windows, in `epik-ui` and `ui/`, only place what these give them.
//! Everything a page says is decided here, in words, so it is tested
//! without a desktop: the window describes, in sentences, what is
//! happening, and every issue or pull request it mentions links to GitHub.

use chrono::{DateTime, FixedOffset, Utc};
use epik_core::agent::CODEX_ACP;
use epik_core::graph::Graph;
use epik_core::hosted::{HostedRun, Shown};
use epik_core::hosts::{Host, State};
use epik_core::record::{BuildStatus, IssueState, IssueStatus, Kind};
use epik_core::remote::Mismatch;
use epik_core::repositories::Repository;
use epik_core::run::{BuildSummary, Disposition, RunState};
use epik_core::setup::{Check, Item};
use epik_core::uninstall::Outcome;
use epik_core::words::{cost_of, dollars, duration, total_cost};
use epik_view::{
    Home, HostRow, HostStatus, IssueCard, Layout, MiniGraph, NodeState, NowLine, Popover, Progress,
    RepositoryRow, RunCard, RunPage, Settings, SetupFix, Sidebar, SidebarRun, Span, TrayRun,
    UninstallStep, Words,
};

/// How long an agent may log nothing and still be "active".
const ACTIVE_FOR: i64 = 60;

/// A run's disposition, as the windows are given it.
fn disposition(disposition: Disposition) -> epik_view::Disposition {
    match disposition {
        Disposition::Building => epik_view::Disposition::Building,
        Disposition::Reviewing => epik_view::Disposition::Reviewing,
        Disposition::WaitingForReview => epik_view::Disposition::WaitingForReview,
        Disposition::NeedsAttention => epik_view::Disposition::NeedsAttention,
    }
}

fn text(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        url: None,
        code: false,
    }
}

fn link(text: impl Into<String>, url: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        url: Some(url.into()),
        code: false,
    }
}

fn code(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        url: None,
        code: true,
    }
}

/// The plain text of `words`, for tests and tooltips.
pub fn plain(words: &[Span]) -> String {
    words.iter().map(|span| span.text.as_str()).collect()
}

/// An issue's page on GitHub.
pub fn issue_url(repo: &str, number: u64) -> String {
    format!("https://github.com/{repo}/issues/{number}")
}

/// A pull request's page on GitHub.
pub fn pull_url(repo: &str, number: u64) -> String {
    format!("https://github.com/{repo}/pull/{number}")
}

/// `#n` linked to issue `n`.
fn issue_link(repo: &str, number: u64) -> Span {
    link(format!("#{number}"), issue_url(repo, number))
}

/// `PR #n` linked to pull request `n`.
fn pull_link(repo: &str, number: u64) -> Span {
    link(format!("PR #{number}"), pull_url(repo, number))
}

/// `items` joined as English does: "a", "a and b", "a, b and c".
fn and_list(items: Vec<Words>) -> Words {
    let count = items.len();
    let mut words = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            words.push(text(if i + 1 == count { " and " } else { ", " }));
        }
        words.extend(item);
    }
    words
}

/// A small number in words, as a sentence starts: "One", "Two", … "Ten",
/// then digits.
fn number_word(n: usize) -> String {
    const WORDS: [&str; 11] = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |word| (*word).to_owned())
}

/// "first", "second", … for a build's place in its run.
fn ordinal(n: usize) -> String {
    const WORDS: [&str; 10] = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
        "tenth",
    ];
    WORDS
        .get(n.wrapping_sub(1))
        .map_or_else(|| format!("{n}th"), |word| (*word).to_owned())
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn lowercased_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What this machine is called in a sentence.
fn this_machine() -> &'static str {
    if cfg!(target_os = "macos") {
        "this Mac"
    } else if cfg!(windows) {
        "this PC"
    } else {
        "this computer"
    }
}

/// How long from `then` to `now`, in words: "12m 00s".
fn since(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    duration(u64::try_from((now - then).num_milliseconds()).unwrap_or(0))
}

/// Where `run` builds, when there are remote hosts: "Building on devbox"
/// or "Built on this Mac", and for a host that couldn't be reached when
/// last asked, or whose app is another version, when it was last heard
/// from.
fn place(run: &HostedRun, shown: &Shown, now: DateTime<Utc>) -> Option<String> {
    let building = run.state.disposition.is_building();
    let verb = if building { "Building" } else { "Built" };
    let Some(host) = &run.host else {
        return (!shown.hosts.is_empty()).then(|| format!("{verb} on {}", this_machine()));
    };
    let mut place = format!("{verb} on {}", host.destination);
    if let Some(State::Unreachable { .. } | State::Mismatch { .. }) = host.state {
        match host.heard_at {
            Some(at) => place.push_str(&format!(", last heard from {} ago.", since(at, now))),
            None => place.push_str(", which hasn't answered yet."),
        }
        if building {
            place.push_str(" The build may still be going on there.");
        }
    }
    Some(place)
}

/// "active 2s ago" while the agent logged within the last minute, else
/// "quiet 1m 50s".
pub fn life(last_event: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let since = (now - last_event).num_seconds().max(0);
    let words = duration(since as u64 * 1000);
    if since <= ACTIVE_FOR {
        format!("active {words} ago")
    } else {
        format!("quiet {words}")
    }
}

fn is_merged(issue: &IssueState) -> bool {
    matches!(issue.status, IssueStatus::Merged | IssueStatus::Closed)
}

fn progress(run: &RunState) -> Progress {
    let count = |wanted: fn(&IssueState) -> bool| run.issues.iter().filter(|i| wanted(i)).count();
    let merged = count(is_merged);
    let running = count(|i| i.status == IssueStatus::Running);
    Progress {
        merged,
        running,
        not_started: run.issues.len() - merged - running,
    }
}

fn node_state(issue: &IssueState) -> NodeState {
    match issue.status {
        IssueStatus::Merged | IssueStatus::Closed => NodeState::Merged,
        IssueStatus::Running => NodeState::Running,
        IssueStatus::Failed => NodeState::Failed,
        // An agent's pull request that waits for review, a single issue's.
        IssueStatus::Succeeded => NodeState::Merged,
        IssueStatus::Waiting => NodeState::NotStarted,
    }
}

/// Lays out `issues`. Issues on or behind a cycle go in a column after the
/// rest.
pub fn layout(issues: &[IssueState]) -> Layout {
    let graph = Graph::new(
        issues
            .iter()
            .map(|m| (m.issue.number, m.issue.blocked_by.iter().copied())),
    );
    let levels = graph.levels();
    let level = |number: u64| levels.get(&number).copied().flatten();
    let after = issues
        .iter()
        .filter_map(|m| level(m.issue.number))
        .max()
        .map_or(0, |deepest| deepest + 1);
    let mut heights: Vec<usize> = Vec::new();
    let places = issues
        .iter()
        .map(|m| {
            let column = level(m.issue.number).unwrap_or(after);
            if heights.len() <= column {
                heights.resize(column + 1, 0);
            }
            let row = heights[column];
            heights[column] += 1;
            (m.issue.number, column, row)
        })
        .collect();
    let is_issue = |n: u64| issues.iter().any(|m| m.issue.number == n);
    let edges = issues
        .iter()
        .flat_map(|m| {
            m.issue
                .blocked_by
                .iter()
                .filter(|&&blocker| is_issue(blocker))
                .map(|&blocker| (blocker, m.issue.number))
        })
        .collect();
    Layout {
        columns: heights.len(),
        rows: heights.iter().copied().max().unwrap_or(0),
        places,
        edges,
    }
}

fn mini_graph(run: &RunState) -> MiniGraph {
    MiniGraph {
        layout: layout(&run.issues),
        states: run.issues.iter().map(node_state).collect(),
    }
}

/// What a running issue is doing: "running cargo test. Last result: 23
/// passed, 1 failed." or "editing tests/ai.rs. No tests run yet."
fn doing(issue: &IssueState) -> Words {
    let Some(activity) = &issue.activity else {
        return vec![text("starting.")];
    };
    let action = activity
        .last_action
        .as_deref()
        .map_or_else(|| "starting".to_owned(), lowercased_first);
    let tests = match &activity.last_test {
        Some(result) => format!("Last result: {result}."),
        None => "No tests run yet.".to_owned(),
    };
    vec![text(format!("{action}. {tests}"))]
}

fn now_line(run: &RunState, issue: &IssueState, now: DateTime<Utc>) -> NowLine {
    let last = issue.activity.as_ref().and_then(|a| a.last_event);
    let mut words = vec![
        issue_link(&run.repo, issue.issue.number),
        text(format!(" {} — ", issue.issue.title)),
    ];
    words.extend(doing(issue));
    NowLine {
        number: issue.issue.number,
        words,
        life: last.map(|at| life(at, now)),
        last_event: last.map(|at| at.timestamp_millis()),
    }
}

fn count_words(run: &RunState) -> String {
    let issues = plural(run.issues.len(), "issue", "issues");
    let merged = run.issues.iter().filter(|i| is_merged(i)).count();
    if run.issues.len() > 1 {
        format!("{issues}, {merged} merged")
    } else {
        issues
    }
}

/// `#14 and #15`, linked.
fn issue_list(run: &RunState, issues: &[&IssueState]) -> Words {
    and_list(
        issues
            .iter()
            .map(|i| vec![issue_link(&run.repo, i.issue.number)])
            .collect(),
    )
}

fn next_words(run: &RunState) -> Option<Words> {
    let waiting: Vec<&IssueState> = run
        .issues
        .iter()
        .filter(|i| i.status == IssueStatus::Waiting)
        .collect();
    if waiting.is_empty() {
        return None;
    }
    let mut words = issue_list(run, &waiting);
    words.push(text(if waiting.len() == 1 {
        " starts when the issues it depends on are merged."
    } else {
        " start when the issues they depend on are merged."
    }));
    Some(words)
}

fn done_words(run: &RunState) -> Option<Words> {
    let merged: Vec<&IssueState> = run.issues.iter().filter(|i| is_merged(i)).collect();
    if merged.is_empty() {
        return None;
    }
    let mut words = issue_list(run, &merged);
    words.push(text(if merged.len() == 1 {
        " is merged into "
    } else {
        " are merged into "
    }));
    words.push(code(&run.branch));
    let pulls: Vec<Words> = merged
        .iter()
        .filter_map(|i| i.pull_request)
        .map(|pr| vec![pull_link(&run.repo, pr)])
        .collect();
    if !pulls.is_empty() {
        words.push(text(" ("));
        for (i, pull) in pulls.into_iter().enumerate() {
            if i > 0 {
                words.push(text(", "));
            }
            words.extend(pull);
        }
        words.push(text(")"));
    }
    words.push(text("."));
    Some(words)
}

/// Why a run needs attention, in words: "stopped: CI failed on PR #8." or
/// "interrupted when Epik quit, with 1 of 3 issues merged."
fn attention_words(run: &RunState) -> Words {
    let latest = run.builds.last();
    let merged = run.issues.iter().filter(|i| is_merged(i)).count();
    let of = format!(
        "{merged} of {}",
        plural(run.issues.len(), "issue", "issues")
    );
    match (latest.map(|b| b.status), run.pull_request) {
        (Some(BuildStatus::Succeeded), Some(pr)) => vec![
            text("its "),
            pull_link(&run.repo, pr),
            text(" was closed without merging."),
        ],
        (Some(BuildStatus::Interrupted), _) => {
            let why = match run.reason.as_deref() {
                Some("cancelled") | None => "interrupted".to_owned(),
                Some(reason) if reason.contains("Epik app stopped") => {
                    "interrupted when Epik quit".to_owned()
                }
                Some(reason) => format!("interrupted ({reason})"),
            };
            vec![text(format!("{why}, with {of} merged."))]
        }
        _ => vec![text(format!(
            "stopped: {}.",
            run.reason
                .as_deref()
                .unwrap_or("the build failed")
                .trim_end_matches('.')
        ))],
    }
}

/// The run's card at `now`, as if it were on this computer.
pub fn run_card(run: &RunState, now: DateTime<Utc>) -> RunCard {
    let building = run.disposition.is_building();
    let running: Vec<&IssueState> = run
        .issues
        .iter()
        .filter(|i| i.status == IssueStatus::Running)
        .collect();
    let epic = issue_url(&run.repo, run.number);
    let (sentence, note) = match run.disposition {
        Disposition::Building | Disposition::Reviewing => (None, None),
        Disposition::WaitingForReview => {
            let mut words = Vec::new();
            if run.kind == Kind::Epic {
                words.push(text(format!(
                    "all {} merged. ",
                    plural(run.issues.len(), "issue", "issues")
                )));
            }
            match run.pull_request {
                Some(pr) => {
                    words.push(pull_link(&run.repo, pr));
                    words.push(text(" is ready for your review."));
                }
                None => words.push(text("ready for your review.")),
            }
            let note = format!(
                "When it merges, Epik comments the total time and cost on #{} and this run goes away.",
                run.number
            );
            (Some(words), Some(note))
        }
        Disposition::NeedsAttention => (Some(attention_words(run)), None),
    };
    let summary = building.then(|| {
        let p = progress(run);
        let mut words = Vec::new();
        if run.issues.len() > 1 {
            words.push(text(format!(
                "{} of {} issues merged. ",
                p.merged,
                run.issues.len()
            )));
        }
        match running.len() {
            0 => words.push(text("Starting.")),
            1 if run.issues.len() == 1 => {
                words.push(text(capitalized(&plain(&doing(running[0])))));
            }
            _ => {
                words.extend(issue_list(run, &running));
                words.push(text(if running.len() == 1 {
                    " is in progress."
                } else {
                    " are in progress."
                }));
            }
        }
        words
    });
    RunCard {
        run: run.run.to_string(),
        title: run.title.clone(),
        place: None,
        epic: link(run.run.to_string(), epic),
        count: count_words(run),
        disposition: disposition(run.disposition),
        elapsed: duration(u64::try_from(run.elapsed(now).num_milliseconds()).unwrap_or(0)),
        cost: total_cost(run.cost_usd, run.cost_unreported),
        graph: mini_graph(run),
        progress: progress(run),
        now: running.iter().map(|i| now_line(run, i, now)).collect(),
        next: if building { next_words(run) } else { None },
        done: if building { done_words(run) } else { None },
        sentence,
        note,
        summary,
    }
}

/// "One run building" or "Two runs building".
fn runs_building(n: usize) -> String {
    match n {
        1 => "One run building".to_owned(),
        n => format!("{} runs building", number_word(n)),
    }
}

/// "Two runs building. Keeping this Mac awake until they finish.", for
/// the runs building here and on `hosts`. Only builds here keep this
/// computer awake.
fn headline(here: usize, hosts: &[&str]) -> String {
    let elsewhere = hosts.len();
    let until = match here {
        1 if elsewhere > 0 => "the one here finishes".to_owned(),
        1 => "it finishes".to_owned(),
        n if elsewhere > 0 => format!("the {} here finish", lowercased_first(&number_word(n))),
        _ => "they finish".to_owned(),
    };
    if here > 0 {
        return format!(
            "{}. Keeping {} awake until {until}.",
            runs_building(here + elsewhere),
            this_machine()
        );
    }
    if elsewhere == 0 {
        return "Nothing is building.".to_owned();
    }
    let mut on: Vec<&str> = hosts.to_vec();
    on.sort_unstable();
    on.dedup();
    let on = match on.as_slice() {
        [host] => (*host).to_owned(),
        _ => "other hosts".to_owned(),
    };
    format!(
        "{} on {on}. {} can sleep.",
        runs_building(elsewhere),
        capitalized(this_machine())
    )
}

/// `run`'s card among the runs `shown`, at `now`: named by its host, and
/// saying where it builds.
fn hosted_card(run: &HostedRun, shown: &Shown, now: DateTime<Utc>) -> RunCard {
    RunCard {
        run: run.key().to_string(),
        place: place(run, shown, now),
        ..run_card(&run.state, now)
    }
}

/// Home, for the runs `shown`, newest activity first, at `now`.
pub fn home(shown: &Shown, now: DateTime<Utc>) -> Home {
    let cards = |wanted: &dyn Fn(Disposition) -> bool| -> Vec<RunCard> {
        shown
            .runs
            .iter()
            .filter(|run| wanted(run.state.disposition))
            .map(|run| hosted_card(run, shown, now))
            .collect()
    };
    let building: Vec<&HostedRun> = shown
        .runs
        .iter()
        .filter(|run| run.state.disposition.is_building())
        .collect();
    let here = building.iter().filter(|run| run.host.is_none()).count();
    let hosts: Vec<&str> = building
        .iter()
        .filter_map(|run| run.host.as_ref())
        .map(|host| host.destination.as_str())
        .collect();
    Home {
        headline: headline(here, &hosts),
        building: cards(&Disposition::is_building),
        waiting: cards(&|d| d == Disposition::WaitingForReview),
        attention: cards(&|d| d == Disposition::NeedsAttention),
    }
}

/// The main window's sidebar, for the same runs as Home and the monitored
/// `repositories`, at `now`, for an Epik that started at `started`.
pub fn sidebar(
    shown: &Shown,
    repositories: &[Repository],
    started: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Sidebar {
    let order = [
        Disposition::NeedsAttention,
        Disposition::Building,
        Disposition::Reviewing,
        Disposition::WaitingForReview,
    ];
    let up = u64::try_from((now - started).num_milliseconds()).unwrap_or(0);
    Sidebar {
        runs: order
            .iter()
            .flat_map(|&wanted| {
                shown
                    .runs
                    .iter()
                    .filter(move |run| run.state.disposition == wanted)
            })
            .map(|run| SidebarRun {
                run: run.key().to_string(),
                title: run.state.title.clone(),
                disposition: disposition(run.state.disposition),
            })
            .collect(),
        missing_repository: repositories.iter().any(|r| r.missing),
        status: format!("Epik running · up {}", duration(up)),
    }
}

/// The tray menu, for the same runs as Home, at `now`, for an Epik that
/// started at `started`. Restarting interrupts only the builds here.
pub fn popover(shown: &Shown, started: DateTime<Utc>, now: DateTime<Utc>) -> Popover {
    let building: Vec<&HostedRun> = shown
        .runs
        .iter()
        .filter(|run| run.state.disposition.is_building())
        .collect();
    let agents = building
        .iter()
        .map(|run| progress(&run.state).running)
        .sum();
    let here = building.iter().filter(|run| run.host.is_none()).count();
    let up = u64::try_from((now - started).num_milliseconds()).unwrap_or(0);
    Popover {
        about: format!(
            "{} · {} · up {}",
            plural(shown.runs.len(), "run", "runs"),
            plural(agents, "agent", "agents"),
            duration(up)
        ),
        restart: restart_question(here),
        runs: building
            .iter()
            .map(|run| tray_run(run, shown, now))
            .collect(),
    }
}

/// What the tray menu's Restart asks, with `building` runs building:
/// "Interrupt two builds?".
pub fn restart_question(building: usize) -> String {
    match building {
        0 => "Restart Epik?".to_owned(),
        1 => "Interrupt one build?".to_owned(),
        n => format!("Interrupt {} builds?", lowercased_first(&number_word(n))),
    }
}

/// What the dialog that confirms a restart says, with `building` runs
/// building.
pub fn restart_warning(building: usize) -> String {
    match building {
        0 => "Epik will quit and start again.".to_owned(),
        1 => "Restarting interrupts the build running now. You can rerun it from its page."
            .to_owned(),
        n => format!(
            "Restarting interrupts the {} builds running now. You can rerun each from its page.",
            lowercased_first(&number_word(n))
        ),
    }
}

/// A building run's row in the tray menu, among the runs `shown`, at
/// `now`.
fn tray_run(hosted: &HostedRun, shown: &Shown, now: DateTime<Utc>) -> TrayRun {
    let run = &hosted.state;
    let running: Vec<&IssueState> = run
        .issues
        .iter()
        .filter(|i| i.status == IssueStatus::Running)
        .collect();
    let mut line = Vec::new();
    if running.is_empty() {
        line.push("starting".to_owned());
    } else {
        line.push(format!("building {}", plain(&issue_list(run, &running))));
        if let Some(since) = running.iter().filter_map(|i| i.started_at).min() {
            line.push(duration(
                u64::try_from((now - since).num_milliseconds()).unwrap_or(0),
            ));
        }
    }
    line.push(run.run.to_string());
    TrayRun {
        run: hosted.key().to_string(),
        title: run.title.clone(),
        place: place(hosted, shown, now),
        count: format!("{}/{}", progress(run).merged, run.issues.len()),
        segments: run.issues.iter().map(node_state).collect(),
        line: line.join(" · "),
    }
}

/// "Starts when #16 and #17 are merged", from the issues it waits for.
fn starts_when(run: &RunState, issue: &IssueState) -> Words {
    let waiting: Vec<&IssueState> = issue
        .issue
        .blocked_by
        .iter()
        .filter_map(|n| run.issue(*n))
        .filter(|blocker| !is_merged(blocker))
        .collect();
    if waiting.is_empty() {
        return vec![text("Starts when its build reaches it")];
    }
    let mut words = vec![text("Starts when ")];
    words.extend(issue_list(run, &waiting));
    words.push(text(if waiting.len() == 1 {
        " is merged"
    } else {
        " are merged"
    }));
    words
}

fn figures(issue: &IssueState) -> Option<String> {
    match (issue.duration_ms, cost_of(issue)) {
        (Some(ms), Some(cost)) => Some(format!("{} · {cost}", duration(ms))),
        (Some(ms), None) => Some(duration(ms)),
        _ => None,
    }
}

fn issue_card(
    run: &RunState,
    issue: &IssueState,
    place: (usize, usize),
    now: DateTime<Utc>,
    log: Option<String>,
) -> IssueCard {
    let state = node_state(issue);
    let number = issue.issue.number;
    let activity = issue.activity.as_ref();
    let last = activity.and_then(|a| a.last_event);
    let pr_words = |pr: u64| vec![pull_link(&run.repo, pr)];
    let line = match (state, issue.status) {
        (NodeState::Merged, IssueStatus::Closed) if issue.pull_request.is_none() => {
            vec![text("Closed")]
        }
        (NodeState::Merged, IssueStatus::Succeeded) => match issue.pull_request {
            Some(pr) => {
                let mut words = pr_words(pr);
                words.push(text(" waits for your review"));
                words
            }
            None => vec![text("Done")],
        },
        (NodeState::Merged, _) => match issue.pull_request {
            Some(pr) => {
                let mut words = vec![text("Merged in ")];
                words.extend(pr_words(pr));
                words
            }
            None => vec![text("Merged")],
        },
        (NodeState::Running, _) => {
            let action = activity.and_then(|a| a.last_action.as_deref()).map_or_else(
                || "starting".to_owned(),
                |a| lowercased_first(a.trim_start_matches("Running ")),
            );
            let tests = activity
                .and_then(|a| a.last_test.as_deref())
                .map_or_else(|| "no tests yet".to_owned(), |t| format!("last: {t}"));
            vec![text(format!("{action} · {tests}"))]
        }
        (NodeState::NotStarted, _) => starts_when(run, issue),
        (NodeState::Failed, _) => vec![text(format!(
            "Failed: {}",
            issue.reason.as_deref().unwrap_or("its agent stopped")
        ))],
    };
    let sentence = match state {
        NodeState::Running => {
            let mut words = vec![text(capitalized(&plain(&doing(issue))))];
            if let Some(life) = last.map(|at| life(at, now)) {
                words.push(text(format!(" Its agent is {life}.")));
            }
            words
        }
        NodeState::NotStarted => {
            let mut words = starts_when(run, issue);
            words.push(text("."));
            words
        }
        _ => {
            let mut words = line.clone();
            words.push(text("."));
            words
        }
    };
    let worked = match state {
        NodeState::Running => {
            let spent = issue
                .started_at
                .map(|at| duration(u64::try_from((now - at).num_milliseconds()).unwrap_or(0)));
            let cost = activity.and_then(|a| a.cost_usd).map(|c| {
                let cents = rust_decimal::Decimal::from_f64_retain(c).unwrap_or_default();
                format!("{} so far", dollars(cents))
            });
            match (spent, cost) {
                (Some(spent), Some(cost)) => Some(format!("{spent} · {cost}")),
                (spent, cost) => spent.or(cost),
            }
        }
        _ => figures(issue),
    };
    let depends_on = {
        let blockers: Vec<&IssueState> = issue
            .issue
            .blocked_by
            .iter()
            .filter_map(|n| run.issue(*n))
            .collect();
        if blockers.is_empty() {
            vec![text("Nothing")]
        } else {
            issue_list(run, &blockers)
        }
    };
    let unblocks = {
        let blocked: Vec<&IssueState> = run
            .issues
            .iter()
            .filter(|i| i.issue.blocked_by.contains(&number))
            .collect();
        if blocked.is_empty() {
            vec![text("Nothing")]
        } else {
            issue_list(run, &blocked)
        }
    };
    let pull_request = match issue.pull_request {
        Some(pr) => pr_words(pr),
        None if state == NodeState::Running => vec![text("Opens when the tests pass")],
        None => vec![text("None yet")],
    };
    IssueCard {
        number,
        url: issue_url(&run.repo, number),
        title: issue.issue.title.clone(),
        state,
        column: place.0,
        row: place.1,
        figures: if state == NodeState::Running {
            None
        } else {
            figures(issue)
        },
        line,
        life: last
            .filter(|_| state == NodeState::Running)
            .map(|at| life(at, now)),
        last_event: last.map(|at| at.timestamp_millis()),
        sentence,
        worked,
        depends_on,
        unblocks,
        pull_request,
        recent: activity.map(|a| a.recent.clone()).unwrap_or_default(),
        log,
    }
}

/// A build in a sentence: "Build 2, running since 20:31, has merged #15 so
/// far" or "Build 1, Oct 27, merged #14, then was interrupted · 12m 00s ·
/// $1.01".
fn build_sentence(n: usize, build: &BuildSummary, merged: &[u64], zone: &FixedOffset) -> String {
    let at = build.started_at.with_timezone(zone);
    let merged = match merged {
        [] => None,
        numbers => Some(
            numbers
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", "),
        ),
    };
    let figures = format!(
        "{} · {}",
        duration(build.elapsed_ms),
        total_cost(build.cost_usd, build.cost_unreported)
    );
    match build.status {
        BuildStatus::Running => match merged {
            Some(merged) => format!(
                "Build {n}, running since {}, has merged {merged} so far",
                at.format("%H:%M")
            ),
            None => format!("Build {n}, running since {}", at.format("%H:%M")),
        },
        status => {
            let ended = match status {
                BuildStatus::Succeeded => match build.pull_request {
                    Some(pr) => format!("opened PR #{pr}"),
                    None => "succeeded".to_owned(),
                },
                BuildStatus::Failed => format!(
                    "failed: {}",
                    build.reason.as_deref().unwrap_or("the build failed")
                ),
                _ => "was interrupted".to_owned(),
            };
            let day = at.format("%b %-d");
            match merged {
                Some(merged) => {
                    format!("Build {n}, {day}, merged {merged}, then {ended} · {figures}")
                }
                None => format!("Build {n}, {day}, {ended} · {figures}"),
            }
        }
    }
}

/// The run page for `hosted` among the runs `shown` at `now`, with the
/// directory of its running build's record, where agents' logs are.
pub fn run_page(
    hosted: &HostedRun,
    shown: &Shown,
    merged_by_build: &[Vec<u64>],
    log_dir: Option<&std::path::Path>,
    now: DateTime<Utc>,
    zone: &FixedOffset,
) -> RunPage {
    let run = &hosted.state;
    let layout = layout(&run.issues);
    let p = progress(run);
    let total = run.issues.len();
    let sentence = match run.disposition {
        Disposition::Building => {
            let build = capitalized(&ordinal(run.builds.len()));
            let agents = match p.running {
                0 => "starting".to_owned(),
                1 => "1 agent working".to_owned(),
                n => format!("{n} agents working"),
            };
            if total > 1 {
                format!(
                    "{build} build running. {} of {total} issues merged, {agents}.",
                    p.merged
                )
            } else {
                format!("{build} build running, {agents}.")
            }
        }
        Disposition::Reviewing => {
            let build = capitalized(&ordinal(run.builds.len()));
            format!("{build} build running, reviewing its pull request.")
        }
        _ => capitalized(&plain(&run_card(run, now).sentence.unwrap_or_default())),
    };
    let mut about = vec![text(format!("{} · ", plural(total, "issue", "issues")))];
    if run.kind == Kind::Epic {
        about.push(text("merging into "));
    } else {
        about.push(text("into "));
    }
    about.push(code(&run.branch));
    let issues = run
        .issues
        .iter()
        .zip(&layout.places)
        .map(|(issue, (_, column, row))| {
            let log = log_dir
                .filter(|_| issue.status == IssueStatus::Running)
                .map(|dir| {
                    let build_id = run.running_build.as_deref().unwrap_or_default();
                    dir.join(issue.log(build_id)).display().to_string()
                });
            issue_card(run, issue, (*column, *row), now, log)
        })
        .collect();
    let builds = run
        .builds
        .iter()
        .enumerate()
        .rev()
        .map(|(i, build)| {
            let merged = merged_by_build
                .get(i)
                .map(Vec::as_slice)
                .unwrap_or_default();
            build_sentence(i + 1, build, merged, zone)
        })
        .collect();
    RunPage {
        run: hosted.key().to_string(),
        title: run.title.clone(),
        place: place(hosted, shown, now),
        epic: link(run.run.to_string(), issue_url(&run.repo, run.number)),
        about,
        sentence,
        disposition: disposition(run.disposition),
        elapsed: duration(u64::try_from(run.elapsed(now).num_milliseconds()).unwrap_or(0)),
        cost: total_cost(run.cost_usd, run.cost_unreported),
        also_building: shown
            .runs
            .iter()
            .filter(|o| o.state.disposition.is_building() && o.key() != hosted.key())
            .map(|o| (o.key().to_string(), o.state.title.clone()))
            .collect(),
        builds,
        columns: layout.columns,
        rows: layout.rows,
        issues,
        edges: layout.edges,
    }
}

/// Settings for the monitored `repositories` and all `runs` here, and
/// the remote `hosts`, each with the runs in its mirror, of which
/// `default` is where builds go, or this computer for none.
pub fn settings(
    repositories: &[Repository],
    runs: &[RunState],
    hosts: &[(Host, Vec<RunState>)],
    default: Option<&str>,
    zone: &FixedOffset,
) -> Settings {
    Settings {
        hosts: std::iter::once(this_host(default.is_none()))
            .chain(
                hosts
                    .iter()
                    .map(|(host, runs)| host_row(host, runs, default)),
            )
            .collect(),
        repositories: repositories
            .iter()
            .map(|repository| {
                let mine: Vec<(String, String)> = runs
                    .iter()
                    .filter(|run| run.repo.eq_ignore_ascii_case(&repository.repo))
                    .map(|run| (run.run.to_string(), run.title.clone()))
                    .collect();
                let count = match mine.len() {
                    0 => "No runs".to_owned(),
                    n => plural(n, "run", "runs"),
                };
                let by = if repository.added_by_user {
                    "by you, "
                } else {
                    ""
                };
                let day = repository.added_at.with_timezone(zone).format("%b %-d");
                RepositoryRow {
                    repo: repository.repo.clone(),
                    url: format!("https://github.com/{}", repository.repo),
                    about: format!("{count} · added {by}{day}"),
                    runs: mine,
                    missing: repository.missing,
                }
            })
            .collect(),
    }
}

/// This computer, as Settings lists it.
fn this_host(default: bool) -> HostRow {
    HostRow {
        destination: None,
        name: "This computer".to_owned(),
        status: HostStatus::Ready,
        about: format!("Epik {}", epik_core::version()),
        why: None,
        missing: Vec::new(),
        default,
        runs: Vec::new(),
    }
}

/// The remote `host`, with the `runs` in its mirror, as Settings lists it.
fn host_row(host: &Host, runs: &[RunState], default: Option<&str>) -> HostRow {
    let missing: Vec<SetupFix> = host
        .setup
        .iter()
        .flatten()
        .filter(|check| check.blocks_build())
        .map(|check| SetupFix {
            title: check.title.clone(),
            detail: check.detail.clone(),
            commands: host_commands(check),
        })
        .collect();
    let (status, heard, why) = match &host.state {
        None => (HostStatus::Unchecked, "Not checked yet".to_owned(), None),
        Some(State::Reachable { version, .. }) => {
            let setup = match &host.setup {
                None => "setup not checked",
                Some(_) if missing.is_empty() => "setup complete",
                Some(_) => "setup incomplete",
            };
            let status = if missing.is_empty() {
                HostStatus::Ready
            } else {
                HostStatus::Incomplete
            };
            (status, format!("Epik {version} · {setup}"), None)
        }
        Some(State::Unreachable { error, .. }) => (
            HostStatus::Unreachable,
            "Unreachable".to_owned(),
            Some(error.clone()),
        ),
        Some(State::Mismatch { version, .. }) => {
            let mismatch = Mismatch {
                destination: host.destination.clone(),
                version: version.clone(),
            };
            (
                HostStatus::Mismatch,
                format!("Epik {version}"),
                Some(mismatch.to_string()),
            )
        }
    };
    let count = match runs.len() {
        0 => "No runs".to_owned(),
        n => plural(n, "run", "runs"),
    };
    HostRow {
        destination: Some(host.destination.clone()),
        name: host.destination.clone(),
        status,
        about: format!("{heard} · {count}"),
        why,
        // An unreachable host's setup may have changed since it was said.
        missing: if status == HostStatus::Incomplete {
            missing
        } else {
            Vec::new()
        },
        default: default == Some(host.destination.as_str()),
        runs: runs
            .iter()
            .map(|run| (run.run.to_string(), run.title.clone()))
            .collect(),
    }
}

/// What to run on a host, which has no window, for the item `check` says
/// is missing there: the tools' own sign-ins, `epik-app setup` for the
/// tools Epik installs, and the Codex the adapter brings, signing in
/// without a browser.
fn host_commands(check: &Check) -> Vec<String> {
    let codex = |args: &str| vec![format!("npx -y -p {CODEX_ACP} codex {args}")];
    let commands: &[&str] = match check.item {
        Item::Gh | Item::Node => {
            return vec![format!(
                "~/.epik/bin/epik-app setup install {}",
                check.item.id()
            )];
        }
        Item::GhAuth if check.detail == "git isn't set to use gh to push" => &["gh auth setup-git"],
        Item::GhAuth => &["gh auth login -s project", "gh auth setup-git"],
        Item::Claude => &["curl -fsSL https://claude.ai/install.sh | bash"],
        Item::ClaudeAuth => &["claude auth login"],
        Item::Codex => return codex("--version"),
        Item::CodexAuth => return codex("login --device-auth"),
        Item::Plugin => &[],
    };
    commands
        .iter()
        .map(|command| (*command).to_owned())
        .collect()
}

/// How each step of uninstalling went, as its dialog lists them.
pub fn uninstall_steps(outcomes: &[Outcome]) -> Vec<UninstallStep> {
    outcomes
        .iter()
        .map(|outcome| UninstallStep {
            title: outcome.step.title().to_owned(),
            error: outcome.result.clone().err(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
