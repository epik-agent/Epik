//! What the tray shows, as a pure function of the run listing.
//!
//! [`menu`] decides the tray's icon and the items of its menu from the runs
//! under `~/.epik/runs` and the runs the app is building. The tray only
//! draws what it says.

use epik_core::run::{Disposition, RunKey, RunState};

/// How many runs not building the menu lists.
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
        run: RunKey,
        label: String,
    },
    /// Text that can't be chosen.
    Note(String),
    Separator,
    OpenRunsFolder,
    Quit,
}

/// The tray for `runs`, newest activity first as
/// [`epik_core::run::list`] gives them, while the runs `active` are
/// building: the building runs, then the [`RECENT`] most recent others,
/// then "Open runs folder" and "Quit".
pub fn menu(runs: &[RunState], active: &[RunKey]) -> Menu {
    let building = |run: &RunState| run.disposition == Disposition::Building;
    let mut items: Vec<Item> = runs
        .iter()
        .filter(|run| building(run))
        .map(run_item)
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
    let others: Vec<Item> = runs
        .iter()
        .filter(|run| !building(run))
        .take(RECENT)
        .map(run_item)
        .collect();
    if !others.is_empty() {
        items.extend(others);
        items.push(Item::Separator);
    }
    items.extend([Item::OpenRunsFolder, Item::Quit]);
    Menu {
        building: !active.is_empty(),
        items,
    }
}

fn run_item(run: &RunState) -> Item {
    let state = match run.disposition {
        Disposition::Building => "building",
        Disposition::WaitingForReview => "waiting for review",
        Disposition::NeedsAttention => "needs attention",
    };
    Item::Run {
        run: run.run.clone(),
        label: format!("{} {} ({state})", run.run, run.title),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use epik_core::record::Kind;
    use rust_decimal::Decimal;

    fn key(number: u64) -> RunKey {
        RunKey::new("o/r", number)
    }

    fn run(number: u64, disposition: Disposition) -> RunState {
        let at = Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap();
        RunState {
            run: key(number),
            repo: "o/r".to_owned(),
            number,
            title: format!("Issue {number}"),
            kind: Kind::Epic,
            branch: format!("epic-{number}"),
            disposition,
            reason: None,
            pull_request: None,
            issues: Vec::new(),
            builds: Vec::new(),
            running_build: None,
            started_at: at,
            latest_at: at,
            elapsed_ms: 0,
            cost_usd: Decimal::ZERO,
        }
    }

    fn item(number: u64, state: &str) -> Item {
        Item::Run {
            run: key(number),
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
    fn it_lists_building_runs_then_the_others_with_their_states() {
        let runs = [
            run(3, Disposition::WaitingForReview),
            run(4, Disposition::Building),
            run(2, Disposition::NeedsAttention),
        ];

        assert_eq!(
            menu(&runs, &[key(4)]),
            Menu {
                building: true,
                items: vec![
                    item(4, "building"),
                    Item::Separator,
                    item(3, "waiting for review"),
                    item(2, "needs attention"),
                    Item::Separator,
                    Item::OpenRunsFolder,
                    Item::Quit,
                ],
            }
        );
    }

    #[test]
    fn it_lists_only_the_ten_most_recent_other_runs() {
        let runs: Vec<RunState> = (1..=12)
            .rev()
            .map(|number| run(number, Disposition::WaitingForReview))
            .collect();

        let items = menu(&runs, &[]).items;

        let listed: Vec<&Item> = items
            .iter()
            .filter(|item| matches!(item, Item::Run { .. }))
            .collect();
        assert_eq!(listed.len(), RECENT);
        assert_eq!(listed[0], &item(12, "waiting for review"));
        assert_eq!(listed[9], &item(3, "waiting for review"));
    }

    #[test]
    fn a_build_not_yet_in_the_listing_still_shows_as_building() {
        let menu = menu(&[], &[key(1)]);

        assert!(menu.building);
        assert_eq!(menu.items[0], note("Starting a build…"));
    }
}
