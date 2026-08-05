//! Light and dark.
//!
//! Theming retrofits badly, so the window has both from its first component and
//! the toggle exists before anything worth looking at does. The two things
//! worth testing about it — which theme follows which, and what the document
//! has to say to be in one — are pure, so they are here rather than in the
//! component that applies them.

/// Which palette the window is wearing.
///
/// Dark by default: the dark palette in `brand.json` has been sitting unused
/// while the light-only website consumed the other one, and this is its first
/// consumer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// The other one. A toggle is this and nothing else.
    #[must_use]
    pub const fn flipped(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    /// What `data-theme` on the document element is set to. Every token in
    /// `styles/tokens.css` is defined under one of exactly these two values.
    #[must_use]
    pub const fn attribute(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// What the toggle is offering. A button is a verb, so its label names
    /// where it goes rather than where it is.
    #[must_use]
    pub const fn invitation(self) -> &'static str {
        match self {
            Self::Dark => "Light",
            Self::Light => "Dark",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_opens_dark() {
        assert_eq!(Theme::default(), Theme::Dark);
    }

    #[test]
    fn flipping_twice_is_where_it_started() {
        for theme in [Theme::Dark, Theme::Light] {
            assert_eq!(theme.flipped().flipped(), theme);
            assert_ne!(theme.flipped(), theme);
        }
    }

    #[test]
    fn each_theme_names_the_selector_its_tokens_are_defined_under() {
        assert_eq!(Theme::Dark.attribute(), "dark");
        assert_eq!(Theme::Light.attribute(), "light");
    }

    #[test]
    fn the_toggle_offers_the_theme_it_would_switch_to() {
        assert_eq!(Theme::Dark.invitation(), "Light");
        assert_eq!(Theme::Light.invitation(), "Dark");
    }
}
