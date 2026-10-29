//! What the tray shows, as a pure function of the run listing.
//!
//! [`menu`] decides the tray's icon and the items of its menu from the runs
//! under `~/.epik/runs` and the builds the app is running. The tray only
//! draws what it says.

use epik_core::record::{BuildState, BuildStatus};

/// How many finished runs the menu lists.
pub const RECENT: usize = 10;

/// What the tray shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// Whether anything is building, which picks the icon.
    pub building: bool,
    pub items: Vec<Item>,
}

/// An item in the tray's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A run; choosing it opens the run.
    Run {
        build_id: String,
        label: String,
    },
    /// Text that can't be chosen.
    Note(String),
    Separator,
    OpenRunsFolder,
    Quit,
}

/// The tray for `runs`, newest first as [`epik_core::record::list_builds`]
/// gives them, while the builds `active` are running: the active runs, then
/// the [`RECENT`] most recent finished ones, then "Open runs folder" and
/// "Quit".
///
/// A run whose record says it is running but which isn't active is one an
/// earlier app was running when it stopped, so it is listed as interrupted.
pub fn menu(runs: &[BuildState], active: &[String]) -> Menu {
    let is_active = |run: &BuildState| active.contains(&run.build_id);
    let mut items: Vec<Item> = runs
        .iter()
        .filter(|run| is_active(run))
        .map(|run| run_item(run, "running"))
        .collect();
    if items.is_empty() {
        // A build is active before its run is recorded.
        let note = if active.is_empty() {
            "Nothing is building"
        } else {
            "Starting a build…"
        };
        items.push(Item::Note(note.to_owned()));
    }
    items.push(Item::Separator);
    let finished: Vec<Item> = runs
        .iter()
        .filter(|run| !is_active(run))
        .take(RECENT)
        .map(|run| run_item(run, finished_state(run.status)))
        .collect();
    if !finished.is_empty() {
        items.extend(finished);
        items.push(Item::Separator);
    }
    items.extend([Item::OpenRunsFolder, Item::Quit]);
    Menu {
        building: !active.is_empty(),
        items,
    }
}

fn run_item(run: &BuildState, state: &str) -> Item {
    Item::Run {
        build_id: run.build_id.clone(),
        label: format!("{}#{} {} ({state})", run.repo, run.number, run.title),
    }
}

fn finished_state(status: BuildStatus) -> &'static str {
    match status {
        BuildStatus::Succeeded => "succeeded",
        BuildStatus::Failed => "failed",
        BuildStatus::Running | BuildStatus::Interrupted => "interrupted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use epik_core::record::Kind;
    use rust_decimal::Decimal;

    fn run(number: u64, status: BuildStatus) -> BuildState {
        BuildState {
            build_id: format!("o-r-{number}"),
            repo: "o/r".to_owned(),
            number,
            title: format!("Issue {number}"),
            kind: Kind::Epic,
            branch: format!("epic-{number}"),
            status,
            reason: None,
            pull_request: None,
            issues: Vec::new(),
            started_at: Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap(),
            ended_at: None,
            elapsed_ms: 0,
            cost_usd: Decimal::ZERO,
        }
    }

    fn item(number: u64, state: &str) -> Item {
        Item::Run {
            build_id: format!("o-r-{number}"),
            label: format!("o/r#{number} Issue {number} ({state})"),
        }
    }

    fn note(text: &str) -> Item {
        Item::Note(text.to_owned())
    }

    #[test]
    fn with_no_runs_it_says_nothing_is_building() {
        assert_eq!(
            menu(&[], &[]),
            Menu {
                building: false,
                items: vec![
                    note("Nothing is building"),
                    Item::Separator,
                    Item::OpenRunsFolder,
                    Item::Quit,
                ],
            }
        );
    }

    #[test]
    fn it_lists_active_runs_then_finished_ones_with_their_states() {
        let runs = [
            run(4, BuildStatus::Running),
            run(3, BuildStatus::Succeeded),
            run(2, BuildStatus::Failed),
            run(1, BuildStatus::Interrupted),
        ];

        assert_eq!(
            menu(&runs, &["o-r-4".to_owned()]),
            Menu {
                building: true,
                items: vec![
                    item(4, "running"),
                    Item::Separator,
                    item(3, "succeeded"),
                    item(2, "failed"),
                    item(1, "interrupted"),
                    Item::Separator,
                    Item::OpenRunsFolder,
                    Item::Quit,
                ],
            }
        );
    }

    #[test]
    fn an_active_run_comes_first_wherever_it_is_listed() {
        let runs = [run(2, BuildStatus::Succeeded), run(1, BuildStatus::Running)];

        let items = menu(&runs, &["o-r-1".to_owned()]).items;

        assert_eq!(
            items[..3],
            [item(1, "running"), Item::Separator, item(2, "succeeded")]
        );
    }

    #[test]
    fn a_run_recorded_as_running_that_isnt_active_was_interrupted() {
        let items = menu(&[run(1, BuildStatus::Running)], &[]).items;

        assert_eq!(
            items[..3],
            [
                note("Nothing is building"),
                Item::Separator,
                item(1, "interrupted")
            ]
        );
    }

    #[test]
    fn it_lists_only_the_ten_most_recent_finished_runs() {
        let runs: Vec<BuildState> = (1..=12)
            .rev()
            .map(|number| run(number, BuildStatus::Succeeded))
            .collect();

        let items = menu(&runs, &[]).items;

        let listed: Vec<&Item> = items
            .iter()
            .filter(|item| matches!(item, Item::Run { .. }))
            .collect();
        assert_eq!(listed.len(), RECENT);
        assert_eq!(listed[0], &item(12, "succeeded"));
        assert_eq!(listed[9], &item(3, "succeeded"));
    }

    #[test]
    fn a_build_not_yet_in_the_listing_still_shows_as_building() {
        let menu = menu(&[], &["o-r-1".to_owned()]);

        assert!(menu.building);
        assert_eq!(menu.items[0], note("Starting a build…"));
    }
}
