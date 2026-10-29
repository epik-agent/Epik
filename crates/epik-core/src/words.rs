//! Figures in words, as the window and Epik's comments write them.

use rust_decimal::{Decimal, RoundingStrategy};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
