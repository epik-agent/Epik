//! Figures in words, as the window and Epik's comments write them.

use rust_decimal::{Decimal, RoundingStrategy};

use crate::record::IssueState;
use crate::settings;

/// `ms` as whole seconds, minutes and seconds, or hours and minutes:
/// "45s", "3m 05s", "1h 02m".
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {:02}s", seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// US dollars to the cent: "$1.23".
pub fn dollars(amount: Decimal) -> String {
    format!(
        "${:.2}",
        amount.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
    )
}

/// An issue's cost, given the agents, by name, that didn't report theirs:
/// "$0.92", "not reported by Codex", or "$0.92 plus some not reported by
/// Codex". `None` if there is nothing to say.
pub fn issue_cost(cost: Option<Decimal>, not_reported_by: &[&str]) -> Option<String> {
    if not_reported_by.is_empty() {
        return cost.map(dollars);
    }
    let by = format!("not reported by {}", not_reported_by.join(" and "));
    Some(match cost {
        Some(cost) => format!("{} plus some {by}", dollars(cost)),
        None => by,
    })
}

/// `issue`'s cost in words, naming the agents that didn't report theirs,
/// as [`issue_cost`] writes it.
pub fn cost_of(issue: &IssueState) -> Option<String> {
    let names: Vec<String> = issue
        .cost_not_reported_by()
        .into_iter()
        .map(settings::name)
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    issue_cost(issue.cost_usd, &names)
}

/// A total cost that lacks `unreported` issues' costs: "$2.06", or "$2.06
/// plus 1 issue not reported".
pub fn total_cost(cost: Decimal, unreported: usize) -> String {
    match unreported {
        0 => dollars(cost),
        1 => format!("{} plus 1 issue not reported", dollars(cost)),
        n => format!("{} plus {n} issues not reported", dollars(cost)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn amount(text: &str) -> Decimal {
        Decimal::from_str(text).unwrap()
    }

    #[test]
    fn an_issues_cost_says_which_agents_did_not_report_theirs() {
        assert_eq!(issue_cost(Some(amount("0.92")), &[]), Some("$0.92".into()));
        assert_eq!(issue_cost(None, &[]), None);
        assert_eq!(
            issue_cost(None, &["Codex"]),
            Some("not reported by Codex".into())
        );
        assert_eq!(
            issue_cost(Some(amount("0.92")), &["Codex"]),
            Some("$0.92 plus some not reported by Codex".into())
        );
        assert_eq!(
            issue_cost(None, &["Claude Code", "Codex"]),
            Some("not reported by Claude Code and Codex".into())
        );
    }

    #[test]
    fn a_total_counts_the_issues_whose_cost_was_not_reported() {
        assert_eq!(total_cost(amount("2.06"), 0), "$2.06");
        assert_eq!(
            total_cost(amount("2.06"), 1),
            "$2.06 plus 1 issue not reported"
        );
        assert_eq!(
            total_cost(Decimal::ZERO, 3),
            "$0.00 plus 3 issues not reported"
        );
    }

    #[test]
    fn durations_read_in_the_largest_units_that_matter() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(45_999), "45s");
        assert_eq!(duration(60_000), "1m 00s");
        assert_eq!(duration(3_599_000), "59m 59s");
        assert_eq!(duration(3_720_000), "1h 02m");
    }

    #[test]
    fn dollars_are_to_the_cent() {
        assert_eq!(dollars(Decimal::ZERO), "$0.00");
        assert_eq!(dollars(Decimal::from_str("2.345").unwrap()), "$2.35");
        assert_eq!(dollars(Decimal::from(12)), "$12.00");
    }
}
