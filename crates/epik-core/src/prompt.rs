//! What an agent is told: a short header saying what the work is, then
//! Epik's rules, then the procedure for that work.
//!
//! The rules and the procedure are the plugin's own files, embedded, so an
//! agent needs no plugin to follow them. `/epik:issue` reads the same
//! procedure when someone builds an issue by hand.

/// The rules that bind every change to the code.
const RULES: &str = include_str!("../../../plugin/skills/assistant/rules.md");

/// How to implement one issue.
const ISSUE_PROCEDURE: &str = include_str!("../../../plugin/skills/issue/procedure.md");

/// What an agent is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// Implement issue #n.
    Issue(u64),
}

/// The prompt for `work` in `repo`, whose pull request goes into `branch`.
pub fn prompt(repo: &str, work: Work, branch: &str) -> String {
    let (task, procedure) = match work {
        Work::Issue(number) => (
            format!("Implement issue #{number} of the GitHub repository {repo}."),
            ISSUE_PROCEDURE,
        ),
    };
    format!(
        "{task} The target branch is `{branch}`.\n\
         \n\
         You work unattended: no one will answer questions. Decide for \
         yourself, and keep going.\n\
         \n\
         # Epik's rules\n\
         \n\
         {RULES}\n\
         # The procedure\n\
         \n\
         {procedure}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_prompt_says_what_the_work_is_then_the_rules_then_the_procedure() {
        let prompt = prompt("owner/repo", Work::Issue(7), "epic/1-thing");

        let (header, rest) = prompt.split_once("# Epik's rules").unwrap();
        assert!(header.contains("issue #7"), "{header}");
        assert!(header.contains("owner/repo"), "{header}");
        assert!(header.contains("`epic/1-thing`"), "{header}");
        assert!(header.contains("no one will answer questions"), "{header}");
        let (rules, procedure) = rest.split_once("# The procedure").unwrap();
        assert!(rules.contains(RULES), "{rules}");
        assert!(procedure.contains(ISSUE_PROCEDURE), "{procedure}");
    }

    #[test]
    fn the_embedded_files_are_the_rules_and_the_issue_procedure() {
        assert!(RULES.contains("The tests are always green"));
        assert!(ISSUE_PROCEDURE.contains("Closes #<issue number>"));
    }
}
