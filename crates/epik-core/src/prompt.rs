//! What an agent is told: a short header saying what the work is, then
//! Epik's rules, then the procedure for that work.
//!
//! The rules and the procedures are the plugin's own files, embedded, so an
//! agent needs no plugin to follow them. `/epik:issue` and `/epik:review`
//! read the same procedures when someone builds or reviews by hand.

/// The rules that bind every change to the code.
const RULES: &str = include_str!("../../../plugin/skills/assistant/rules.md");

/// How to implement one issue.
const ISSUE_PROCEDURE: &str = include_str!("../../../plugin/skills/issue/procedure.md");

/// How to review a pull request, then how to address a review.
const REVIEW_PROCEDURE: &str = include_str!("../../../plugin/skills/review/procedure.md");

/// The heading of the review procedure's part for the fixer.
const ADDRESSING_A_REVIEW: &str = "# Addressing a review";

/// What an agent is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work<'a> {
    /// Implement issue #n.
    Issue(u64),
    /// Address the review of `pull_request`, open on branch `head`, which
    /// implements `issue`, an epic when `epic` is set. An epic's `head` is
    /// its epic branch.
    Address {
        issue: u64,
        epic: bool,
        pull_request: u64,
        head: &'a str,
    },
}

/// The prompt for `work` in `repo`, whose pull request goes into `branch`.
pub fn prompt(repo: &str, work: Work, branch: &str) -> String {
    let (task, instead, procedure) = match work {
        Work::Issue(number) => (
            format!("Implement issue #{number} of the GitHub repository {repo}."),
            String::new(),
            ISSUE_PROCEDURE.to_owned(),
        ),
        Work::Address {
            issue,
            epic,
            pull_request,
            head,
        } => {
            let (implements, on) = if epic {
                (
                    format!("epic #{issue}"),
                    format!(
                        "the epic branch, `{head}`. Push to it directly, as Epik's own \
                         merges into it are"
                    ),
                )
            } else {
                (format!("issue #{issue}"), format!("branch `{head}`"))
            };
            let (_, addressing) = review_procedure();
            (
                format!(
                    "Address the review of pull request #{pull_request} of the GitHub \
                     repository {repo}, which implements {implements}."
                ),
                format!(
                    "Pull request #{pull_request} is already open, on {on}. This \
                     replaces the procedure's first steps: make the worktree from \
                     `{head}`, not a new branch; there, address each open review thread \
                     as \"Addressing a review\" says; push to `{head}`; and open no new \
                     pull request. Then wait for its checks as the procedure says.\n\
                     \n"
                ),
                format!("{ISSUE_PROCEDURE}\n{addressing}"),
            )
        }
    };
    format!(
        "{task} The target branch is `{branch}`.\n\
         \n\
         {instead}\
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

/// The prompt for round `round` of the review of pull request
/// `pull_request` in `repo`, which implements `issue`, an epic when it has
/// `sub_issues`.
pub fn review_prompt(
    repo: &str,
    issue: u64,
    sub_issues: &[u64],
    pull_request: u64,
    round: u32,
) -> String {
    let implements = if sub_issues.is_empty() {
        format!("issue #{issue}")
    } else {
        let subs: Vec<String> = sub_issues.iter().map(|n| format!("#{n}")).collect();
        format!("epic #{issue}, with sub-issues {}", subs.join(", "))
    };
    let (reviewing, _) = review_procedure();
    format!(
        "Review pull request #{pull_request} of the GitHub repository {repo}, \
         which implements {implements}. This is round {round} of the review.\n\
         \n\
         You work unattended: no one will answer questions. Decide for \
         yourself, and keep going.\n\
         \n\
         # Epik's rules\n\
         \n\
         {RULES}\n\
         # The procedure\n\
         \n\
         {reviewing}"
    )
}

/// The review procedure's two parts: reviewing a pull request, and
/// addressing a review, which starts with its heading.
fn review_procedure() -> (&'static str, &'static str) {
    let at = REVIEW_PROCEDURE
        .find(ADDRESSING_A_REVIEW)
        .expect("the review procedure has a part for addressing a review");
    REVIEW_PROCEDURE.split_at(at)
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

    #[test]
    fn a_review_prompt_says_what_to_review_then_the_rules_then_the_reviewing_procedure() {
        let prompt = review_prompt("owner/repo", 7, &[], 12, 2);

        let (header, rest) = prompt.split_once("# Epik's rules").unwrap();
        assert!(header.contains("pull request #12"), "{header}");
        assert!(header.contains("issue #7"), "{header}");
        assert!(header.contains("owner/repo"), "{header}");
        assert!(header.contains("round 2"), "{header}");
        assert!(header.contains("no one will answer questions"), "{header}");
        assert!(!header.contains("sub-issues"), "{header}");
        let (rules, procedure) = rest.split_once("# The procedure").unwrap();
        assert!(rules.contains(RULES), "{rules}");
        assert!(
            procedure.contains("<!-- epik-review: approve round=<r> -->"),
            "{procedure}"
        );
        assert!(!procedure.contains("# Addressing a review"), "{procedure}");
    }

    #[test]
    fn an_epics_review_prompt_lists_its_sub_issues() {
        let prompt = review_prompt("owner/repo", 1, &[2, 3], 9, 1);

        let (header, _) = prompt.split_once("# Epik's rules").unwrap();
        assert!(header.contains("epic #1"), "{header}");
        assert!(header.contains("sub-issues #2, #3"), "{header}");
        assert!(header.contains("round 1"), "{header}");
    }

    #[test]
    fn an_address_prompt_names_the_pull_request_and_its_branch_then_the_procedures() {
        let work = Work::Address {
            issue: 7,
            epic: false,
            pull_request: 12,
            head: "issue-7-thing",
        };
        let prompt = prompt("owner/repo", work, "main");

        let (header, rest) = prompt.split_once("# Epik's rules").unwrap();
        assert!(header.contains("review of pull request #12"), "{header}");
        assert!(header.contains("issue #7"), "{header}");
        assert!(header.contains("owner/repo"), "{header}");
        assert!(header.contains("`main`"), "{header}");
        assert!(header.contains("`issue-7-thing`"), "{header}");
        assert!(header.contains("open no new pull request"), "{header}");
        assert!(header.contains("no one will answer questions"), "{header}");
        assert!(!header.contains("epic"), "{header}");
        let (rules, procedure) = rest.split_once("# The procedure").unwrap();
        assert!(rules.contains(RULES), "{rules}");
        let (_, addressing) = review_procedure();
        assert!(procedure.contains(ISSUE_PROCEDURE), "{procedure}");
        assert!(procedure.ends_with(addressing), "{procedure}");
        assert!(!procedure.contains("# Reviewing"), "{procedure}");
    }

    #[test]
    fn an_epics_address_prompt_names_the_epic_branch() {
        let work = Work::Address {
            issue: 1,
            epic: true,
            pull_request: 9,
            head: "epic/1-thing",
        };
        let prompt = prompt("owner/repo", work, "main");

        let (header, _) = prompt.split_once("# Epik's rules").unwrap();
        assert!(header.contains("epic #1"), "{header}");
        assert!(
            header.contains("the epic branch, `epic/1-thing`"),
            "{header}"
        );
        assert!(header.contains("Push to it directly"), "{header}");
    }

    #[test]
    fn the_review_procedure_has_a_part_for_the_reviewer_and_one_for_the_fixer() {
        let (reviewing, addressing) = review_procedure();
        assert!(reviewing.contains("<!-- epik-review: changes round=<r> -->"));
        assert!(addressing.starts_with("# Addressing a review"));
        assert!(addressing.contains("resolveReviewThread"));
        // The fixer is given only its part, so it lists the threads itself.
        assert!(addressing.contains("reviewThreads"));
    }
}
