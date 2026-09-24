//! What Epik tells the agents it launches, compiled in.
//!
//! Every prompt is a Markdown file at this crate's root, embedded with
//! `include_crypt!` and named in `build.rs` so that editing it rebuilds
//! the crate. The binary carries each one encrypted, so `strings(1)` on
//! a shipped Epik hands over none of them — obfuscation, not secrecy,
//! in the manner of [the system prompt](crate::assistant::SYSTEM_PROMPT).
//!
//! A file is the standing part of a prompt: what is true of every run of
//! its kind. The function beside it is the rest — the branch, the issue,
//! the checks that went red — and the two together are what one agent
//! reads. Every brief here says the same three things about git: the
//! agent commits, and leaves pushing, branching and git configuration to
//! Epik, which does everything around the code. Each is prose the whole
//! way through, since an Epik-launched agent runs in a worktree of
//! another repository, where `.claude/commands/` is a directory of
//! Epik's own and a slash command would name nothing.

/// The preface of every issue brief: where the agent is, what
/// implementing an issue means here, and what git is Epik's.
static ISSUE: include_crypt::EncryptedFile = include_crypt::include_crypt!("issue-brief.md");

/// The preface of a fix an agent's own session comes back to.
static RESUMED: include_crypt::EncryptedFile = include_crypt::include_crypt!("fix-resumed.md");

/// The preface of a fix briefed from the record rather than from a
/// session that held it.
static FRESH: include_crypt::EncryptedFile = include_crypt::include_crypt!("fix-fresh.md");

/// A compiled-in prompt as the words it was written as, without the
/// newline a text file ends with. Decrypted once, on the first brief
/// that needs it, as [the system prompt](crate::assistant::SYSTEM_PROMPT)
/// is.
fn text(file: &include_crypt::EncryptedFile) -> String {
    let mut decrypted = file.decrypt_str().expect("a prompt decrypts to utf-8");
    decrypted.truncate(decrypted.trim_end().len());
    decrypted
}

/// The words of `ISSUE`, read once.
static ISSUE_TEXT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| text(&ISSUE));

/// The words of `RESUMED`, read once.
static RESUMED_TEXT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| text(&RESUMED));

/// The words of `FRESH`, read once.
static FRESH_TEXT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| text(&FRESH));

/// What one issue's agent is told: the preamble, the branch it is on,
/// then the issue as the tracker holds it. The branch is already
/// checked out in the worktree the agent runs in, and the commit and
/// the push are the build's own steps, after the agent has exited.
#[must_use]
pub fn issue_brief(branch: &str, issue: &str) -> String {
    format!("{}\n\nBranch: {branch}\n\n{issue}", *ISSUE_TEXT)
}

/// What the agent that wrote the branch is told when it is resumed to
/// fix it: the preamble, then the required checks that went red on the
/// commit it pushed. The session it comes back to holds what it built
/// and why, so neither is repeated here.
#[must_use]
pub fn resumed_fix_brief(checks: &str) -> String {
    format!("{}\n\nRed checks: {checks}", *RESUMED_TEXT)
}

/// What a fresh agent is told to fix a branch whose session is gone:
/// the preamble, the branch, the issue, the branch's diff against the
/// feature branch, and the same red checks. Everything the resumed
/// agent would have remembered is written out.
#[must_use]
pub fn fresh_fix_brief(branch: &str, issue: &str, diff: &str, checks: &str) -> String {
    format!(
        "{}\n\nBranch: {branch}\n\nRed checks: {checks}\n\n{issue}\n\n## The branch so far\n\n\
         ```diff\n{diff}\n```",
        *FRESH_TEXT
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every prompt this module compiles in is named in `build.rs`, so
    /// editing one rebuilds the crate. A prompt added here and forgotten
    /// there would ship the words it was compiled from last, and nothing
    /// else would say so.
    #[test]
    fn every_prompt_is_watched_by_the_build_script() {
        let build = include_str!("../build.rs");
        let source = include_str!("prompt.rs");
        let compiled = source
            .split("include_crypt!(\"")
            .skip(1)
            .filter_map(|rest| rest.split_once('"').map(|(name, _)| name));
        for name in compiled {
            assert!(
                build.contains(&format!("cargo:rerun-if-changed={name}")),
                "build.rs does not watch {name}"
            );
        }
    }

    /// The obfuscation round-trips: what ships encrypted decrypts to
    /// exactly the file Bill writes.
    #[test]
    fn every_prompt_decrypts_to_the_file_it_was_compiled_from() {
        assert_eq!(
            ISSUE.decrypt_str().unwrap(),
            include_str!("../issue-brief.md")
        );
        assert_eq!(
            RESUMED.decrypt_str().unwrap(),
            include_str!("../fix-resumed.md")
        );
        assert_eq!(
            FRESH.decrypt_str().unwrap(),
            include_str!("../fix-fresh.md")
        );
    }

    #[test]
    fn the_issue_brief_leads_with_the_preamble_and_names_the_branch_and_the_issue() {
        let brief = issue_brief("issue-7", "# Lay the stone\n\nAnd square it.");
        assert!(
            brief.starts_with("You are working in a git worktree"),
            "{brief}"
        );
        assert!(brief.contains("Do not push"), "{brief}");
        assert!(brief.contains("Branch: issue-7"), "{brief}");
        assert!(brief.contains("# Lay the stone"), "{brief}");
    }

    #[test]
    fn the_resumed_fix_brief_names_the_red_checks() {
        let brief = resumed_fix_brief("build, clippy");
        assert!(brief.contains("Red checks: build, clippy"), "{brief}");
        assert!(
            brief.contains("Reproduce those failures locally"),
            "{brief}"
        );
        assert!(brief.contains("Do not push"), "{brief}");
    }

    #[test]
    fn the_fresh_fix_brief_carries_the_issue_the_diff_and_the_red_checks() {
        let brief = fresh_fix_brief(
            "issue-7",
            "# Lay the stone",
            "--- a/wall.rs\n+++ b/wall.rs",
            "build, clippy",
        );
        assert!(brief.contains("Branch: issue-7"), "{brief}");
        assert!(brief.contains("# Lay the stone"), "{brief}");
        assert!(brief.contains("--- a/wall.rs"), "{brief}");
        assert!(brief.contains("Red checks: build, clippy"), "{brief}");
        assert!(brief.contains("Do not push"), "{brief}");
    }

    /// The rule of `2026-09-20-epik-does-everything-around-the-code`:
    /// an agent Epik launches has no `.claude/commands/`, so a prompt
    /// naming a slash command would name nothing. Every brief is prose,
    /// and a run of non-space characters opening with a slash is what a
    /// slash command looks like.
    #[test]
    fn every_brief_is_prose_the_whole_way_through() {
        for brief in [
            issue_brief("issue-7", "the issue"),
            resumed_fix_brief("build"),
            fresh_fix_brief("issue-7", "the issue", "the diff", "build"),
        ] {
            let commands: Vec<_> = brief
                .split_whitespace()
                .filter(|word| word.starts_with('/'))
                .collect();
            assert!(commands.is_empty(), "{commands:?} in {brief}");
        }
    }
}
