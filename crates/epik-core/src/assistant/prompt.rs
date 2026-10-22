//! The system prompt, compiled in.

static PROMPT: include_crypt::EncryptedFile = include_crypt::include_crypt!("prompt.md");

/// Epik's voice: the system prompt that leads every turn, whoever the
/// host is — the desktop app today, a headless daemon tomorrow. Written
/// in prompt.md at this crate's root, compiled in encrypted so the binary
/// doesn't hand it to strings(1) — obfuscation, not secrecy — and never
/// part of any transcript. It rides with `native` because every host
/// that speaks is one; the obfuscation machinery doesn't build for wasm.
/// The rules are composed into it at its marker, so the assistant plans
/// by the same words every agent that writes code is bound by.
pub static SYSTEM_PROMPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    compose(
        &PROMPT.decrypt_str().expect("the prompt decrypts to utf-8"),
        &crate::prompt::RULES_TEXT,
    )
});

/// The line in prompt.md that the rules take the place of.
const MARKER: &str = "<!-- rules -->";

/// `prompt` with its one marker replaced by `rules`. A prompt that lost
/// its marker would ship without the rules and nothing else would say
/// so, and one with two would carry them twice, so either panics.
fn compose(prompt: &str, rules: &str) -> String {
    assert_eq!(
        prompt.matches(MARKER).count(),
        1,
        "the prompt holds the rules marker exactly once"
    );
    prompt.replacen(MARKER, rules, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The system prompt is the two files, composed: `prompt.md` with
    /// the text of `rules.md` where its marker was.
    #[test]
    fn the_system_prompt_is_the_prompt_file_with_the_rules_at_its_marker() {
        let prompt = include_str!("../../prompt.md");
        let rules = include_str!("../../rules.md");
        assert_eq!(*SYSTEM_PROMPT, compose(prompt, rules.trim_end()));
        assert_eq!(SYSTEM_PROMPT.matches(rules.trim_end()).count(), 1);
        assert!(!SYSTEM_PROMPT.contains(MARKER), "{}", *SYSTEM_PROMPT);
    }

    #[test]
    fn the_rules_take_the_place_of_the_marker_line() {
        assert_eq!(
            compose("Before.\n\n<!-- rules -->\n\nAfter.\n", "1. Rule."),
            "Before.\n\n1. Rule.\n\nAfter.\n"
        );
    }

    #[test]
    #[should_panic(expected = "exactly once")]
    fn a_prompt_without_the_marker_panics() {
        compose("Before.\n\nAfter.\n", "1. Rule.");
    }

    #[test]
    #[should_panic(expected = "exactly once")]
    fn a_prompt_with_two_markers_panics() {
        compose("<!-- rules -->\n\n<!-- rules -->\n", "1. Rule.");
    }
}
