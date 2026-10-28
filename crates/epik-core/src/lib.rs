//! The logic of the Epik app. Everything here is tested without a desktop.

/// The version of Epik, which every crate in the workspace shares.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_three_dot_separated_numbers() {
        let parts: Vec<&str> = version().split('.').collect();
        assert_eq!(parts.len(), 3, "{}", version());
        assert!(parts.iter().all(|part| part.parse::<u64>().is_ok()));
    }
}
