//! The verdict a review agent leaves on a pull request.
//!
//! The review agent can't approve or request changes on GitHub, since it
//! acts as the user and the pull request is the user's own. Instead its
//! review's summary ends with Epik's verdict marker, naming the round it
//! answers: `<!-- epik-review: approve round=1 -->` or
//! `<!-- epik-review: changes round=1 -->`. The round, not a time, ties a
//! verdict to an attempt, so the building machine's clock and GitHub's
//! never have to agree, and a reader of the pull request sees which
//! review answered which attempt.

use crate::github::Review;

/// What a review concluded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Approve,
    Changes,
}

/// The verdict of the newest review among `reviews` whose body's last
/// non-blank line is a verdict marker naming `round`, or `None` when no
/// review answers that round.
pub fn verdict(reviews: &[Review], round: u32) -> Option<Verdict> {
    reviews
        .iter()
        .filter_map(|review| {
            let (verdict, answers) = marker(&review.body)?;
            (answers == round).then_some((review.submitted_at, verdict))
        })
        // Of reviews submitted the same second, the later listed wins.
        .max_by_key(|(submitted_at, _)| *submitted_at)
        .map(|(_, verdict)| verdict)
}

/// The verdict and round of the marker on `body`'s last non-blank line,
/// if that line is one.
fn marker(body: &str) -> Option<(Verdict, u32)> {
    let line = body.lines().rev().find(|line| !line.trim().is_empty())?;
    let inside = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .strip_prefix("epik-review:")?;
    let mut words = inside.split_whitespace();
    let verdict = match words.next()? {
        "approve" => Verdict::Approve,
        "changes" => Verdict::Changes,
        _ => return None,
    };
    let round = words.next()?.strip_prefix("round=")?.parse().ok()?;
    words.next().is_none().then_some((verdict, round))
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;

    /// A review with `body`, submitted `minute` minutes into the hour.
    fn review(body: &str, minute: u32) -> Review {
        let submitted_at: DateTime<Utc> = format!("2026-11-01T12:{minute:02}:00Z").parse().unwrap();
        Review {
            body: body.to_owned(),
            submitted_at,
        }
    }

    #[test]
    fn an_approving_marker_approves() {
        let body = "Looks right.\n\n<!-- epik-review: approve round=1 -->";
        assert_eq!(verdict(&[review(body, 0)], 1), Some(Verdict::Approve));
    }

    #[test]
    fn a_changes_marker_asks_for_changes() {
        let body = "Two concerns, inline.\n\n<!-- epik-review: changes round=2 -->";
        assert_eq!(verdict(&[review(body, 0)], 2), Some(Verdict::Changes));
    }

    #[test]
    fn trailing_blank_lines_are_skipped() {
        let body = "Fine.\n<!-- epik-review: approve round=1 -->\n\n  \n";
        assert_eq!(verdict(&[review(body, 0)], 1), Some(Verdict::Approve));
    }

    #[test]
    fn a_marker_for_another_round_does_not_answer() {
        let body = "Fine.\n<!-- epik-review: approve round=1 -->";
        assert_eq!(verdict(&[review(body, 0)], 2), None);
        let body = "Fine.\n<!-- epik-review: approve round=12 -->";
        assert_eq!(verdict(&[review(body, 0)], 1), None);
    }

    #[test]
    fn a_marker_not_on_the_last_line_does_not_count() {
        let body = "<!-- epik-review: approve round=1 -->\nOne more thing.";
        assert_eq!(verdict(&[review(body, 0)], 1), None);
    }

    #[test]
    fn the_newest_review_for_the_round_wins() {
        let changes = review("<!-- epik-review: changes round=1 -->", 5);
        let approve = review("<!-- epik-review: approve round=1 -->", 9);
        let other_round = review("<!-- epik-review: changes round=2 -->", 30);
        let unmarked = review("A comment.", 40);
        let reviews = [approve.clone(), changes.clone(), other_round, unmarked];
        assert_eq!(verdict(&reviews, 1), Some(Verdict::Approve));
        assert_eq!(verdict(&[changes, approve], 1), Some(Verdict::Approve));
    }

    #[test]
    fn no_marked_review_is_no_verdict() {
        assert_eq!(verdict(&[], 1), None);
        let reviews = [review("", 0), review("Looks fine to me.", 1)];
        assert_eq!(verdict(&reviews, 1), None);
    }

    #[test]
    fn something_that_only_resembles_a_marker_is_no_verdict() {
        for body in [
            "<!-- epik-review: maybe round=1 -->",
            "<!-- epik-review: approve -->",
            "<!-- epik-review: approve round=one -->",
            "<!-- other-tool: approve round=1 -->",
            "epik-review: approve round=1",
            "<!-- epik-review: approve round=1 extra -->",
        ] {
            assert_eq!(verdict(&[review(body, 0)], 1), None, "{body}");
        }
    }
}
