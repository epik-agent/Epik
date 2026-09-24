//! What the repository's rulesets let a build do, read once at launch.
//!
//! GitHub holds the rules a branch is held to, and enforces them on every
//! merge, Epik's included. A build therefore reads them before it creates
//! anything — the rules on the feature's branch and on each issue's — and
//! obeys what it finds: the checks it must wait for, whether an issue's
//! branch must be current with the feature's, and which merge method the
//! merge may ask for.
//!
//! Epik reads these settings and never writes them. A rule it cannot
//! satisfy — a review it cannot give itself, a merge queue it does not
//! drive, a signature it cannot make, a push it would not be allowed —
//! refuses the build at launch, naming the rule and the ruleset it comes
//! from, so the one who can relax it knows where to go. A rule type this
//! module has not heard of is said and otherwise passed over: GitHub
//! enforces it either way, and a failure it causes is GitHub's to state
//! at the step it blocks.

use crate::dag::Dag;
use crate::github::{Error, GitHub, Kind, Merge, Repo, Required, Rule};
use crate::plan::feature::issue_branch;
use crate::plan::github::feature_branch;

/// What a build obeys, read from the rules in force on its branches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    /// The checks that must pass before the feature's branch takes a
    /// merge. An empty list is a repository that requires none.
    pub checks: Vec<Required>,
    /// Whether an issue's branch must be current with the feature's
    /// branch before GitHub will merge it.
    pub strict: bool,
    /// How a merge asks to be made: squash where the rules allow it, else
    /// a merge commit, else rebase.
    pub merge: Merge,
}

impl Default for Policy {
    /// What a repository with no rulesets asks of a build: no checks, and
    /// a squash merge, which is what GitHub allows where nothing says
    /// otherwise.
    fn default() -> Self {
        Self {
            checks: Vec::new(),
            strict: false,
            merge: Merge::Squash,
        }
    }
}

impl Policy {
    /// The rules in force on feature `number`'s branch and on each
    /// issue of `work`, read into what the build will obey.
    ///
    /// The feature's branch decides the policy: it is the branch the
    /// merges go into, so its rules are the ones a merge must satisfy.
    /// An issue's branch is read for what it would refuse — a signature
    /// Epik cannot make, a push it would not be allowed.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when GitHub cannot be asked or answers no.
    /// A rule Epik cannot satisfy is a refusal inside the [`Reading`]
    /// rather than an error: the launch records it and stops, where an
    /// error means the rules were never learned.
    pub fn read(
        github: &GitHub,
        repo: &Repo,
        number: u64,
        work: &Dag<u64>,
    ) -> Result<Reading, Error> {
        let branch = feature_branch(number);
        let rules = github.branch_rules(repo, &branch)?;
        let mut gathering = Gathering::default();
        let policy = gathering.over_feature(&branch, &rules, parallel(work));
        for issue in work.nodes() {
            let branch = issue_branch(*issue.value());
            let rules = github.branch_rules(repo, &branch)?;
            gathering.over_issue(&branch, &rules);
        }
        Ok(gathering.settle(policy))
    }
}

/// A launch's reading of the rules: the policy it settled on, or the
/// refusal that stops the build, and whatever the reading has to say
/// either way.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reading {
    /// The policy, or the words a refused build is abandoned with.
    pub settled: Result<Policy, String>,
    /// What the reading has to say about rules it obeys but cannot act
    /// on, and rule types it does not know.
    pub said: Vec<String>,
}

/// A reading under way: what would refuse the build and what it has to
/// say, gathered branch by branch before either becomes a [`Reading`].
#[derive(Default)]
struct Gathering {
    refusals: Vec<String>,
    said: Vec<String>,
}

impl Gathering {
    /// The policy the feature's branch dictates, with whatever its rules
    /// refuse or leave unread gathered along the way. `parallel` is
    /// whether two of the build's issues can run at once, which is what
    /// makes a strict rule worth saying.
    fn over_feature(&mut self, branch: &str, rules: &[Rule], parallel: bool) -> Policy {
        let mut policy = Policy::default();
        let mut linear = false;
        let mut methods = Merge::PREFERRED.to_vec();
        for rule in rules {
            match &rule.kind {
                Kind::RequiredStatusChecks { checks, strict } => {
                    // Every rule's checks must pass, so the policy waits
                    // for all of them; a check two rulesets both require
                    // is one check. Strictness is either one's to ask.
                    for required in checks {
                        if !policy.checks.contains(required) {
                            policy.checks.push(required.clone());
                        }
                    }
                    policy.strict |= *strict;
                    if *strict && parallel {
                        self.said.push(format!(
                            "the rules on {branch} are strict ({}): an issue's branch must \
                             be current with {branch} to merge, and issues that run at once \
                             go stale as their siblings land, so the later of two parallel \
                             issues fails at its merge",
                            rule.whence()
                        ));
                    }
                }
                Kind::PullRequest {
                    approvals,
                    methods: allowed,
                    gates,
                } if *approvals == 0 && gates.is_empty() => {
                    // Each rule bounds the merge, so what the build may
                    // ask for is what every rule allows.
                    methods.retain(|method| allowed.contains(method));
                }
                // An approval Epik would have to give its own pull
                // request, asked for either by a count of reviews or by
                // one of the rule's other conditions on a merge.
                Kind::PullRequest { gates, .. } if gates.is_empty() => self.refuse(
                    rule,
                    branch,
                    "an approving review is required, and Epik cannot approve its own \
                     pull requests",
                ),
                Kind::PullRequest { gates, .. } => self.refuse(
                    rule,
                    branch,
                    &format!(
                        "the pull request rule asks for {}, which the one who opened it \
                         cannot give",
                        gates.join(" and ")
                    ),
                ),
                Kind::RequiredLinearHistory => linear = true,
                Kind::MergeQueue => self.refuse(
                    rule,
                    branch,
                    "merging goes through a merge queue, which Epik does not drive",
                ),
                _ => self.over_either(rule, branch),
            }
        }
        if linear {
            methods.retain(|method| *method != Merge::Commit);
        }
        match methods.first() {
            Some(method) => policy.merge = *method,
            None => self.refusals.push(format!(
                "the rules on {branch} leave no merge method Epik could use"
            )),
        }
        policy
    }

    /// What an issue's branch refuses. Its rules decide nothing: the
    /// merges go into the feature's branch, and an issue's branch is only
    /// created and pushed to.
    fn over_issue(&mut self, branch: &str, rules: &[Rule]) {
        for rule in rules {
            match rule.kind {
                Kind::RequiredSignatures => self.refuse(
                    rule,
                    branch,
                    "every commit must be signed, and Epik's commits are unsigned",
                ),
                _ => self.over_either(rule, branch),
            }
        }
    }

    /// What a rule means wherever it stands: the two restrictions that
    /// would stop a build on either branch, and, for everything else, the
    /// report a rule the policy does not read gets.
    fn over_either(&mut self, rule: &Rule, branch: &str) {
        match &rule.kind {
            Kind::Creation => self.refuse(
                rule,
                branch,
                "creating the branch is restricted, and Epik could not create it",
            ),
            Kind::Update => self.refuse(
                rule,
                branch,
                "pushing to the branch is restricted, and Epik could not push",
            ),
            // GitHub enforces the rule whether or not the policy reads
            // it, so a failure it causes is GitHub's to state at the step
            // it blocks.
            kind => self.said.push(format!(
                "the rule {} stands over {branch} ({}), and the build passes it over: \
                 GitHub enforces it either way",
                kind.spelled(),
                rule.whence()
            )),
        }
    }

    /// A rule that stops the build, in words naming what it asks, the
    /// branch it stands over, and the ruleset to relax.
    fn refuse(&mut self, rule: &Rule, branch: &str, because: &str) {
        self.refusals
            .push(format!("on {branch}, {because} ({})", rule.whence()));
    }

    /// The reading the gathering came to: the policy, or every refusal
    /// in one paragraph.
    fn settle(self, policy: Policy) -> Reading {
        let settled = match self.refusals.is_empty() {
            true => Ok(policy),
            false => Err(format!(
                "the repository's rulesets forbid what this build would do: {}",
                self.refusals.join("; ")
            )),
        };
        Reading {
            settled,
            said: self.said,
        }
    }
}

/// Whether two of `work`'s issues can run at once: some issue that has
/// another neither before nor after it, which is to say the work is
/// something other than a single chain.
fn parallel(work: &Dag<u64>) -> bool {
    let total = work.nodes().len();
    work.nodes()
        .any(|issue| issue.ancestors().count() + issue.descendants().count() + 1 < total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::Node;
    use crate::github::fake::{FakeGitHub, Ruleset, branch_rules};
    use serde_json::json;

    const FEATURE: u64 = 274;

    /// A required-checks rule over `contexts`, each `(name, app)`.
    fn checks(contexts: &[(&str, Option<u64>)], strict: bool) -> serde_json::Value {
        let required: Vec<_> = contexts
            .iter()
            .map(|(context, app)| json!({ "context": context, "integration_id": app }))
            .collect();
        json!({
            "type": "required_status_checks",
            "parameters": {
                "strict_required_status_checks_policy": strict,
                "required_status_checks": required,
            },
        })
    }

    /// A pull-request rule asking for `approvals` reviews and allowing
    /// `methods`, in GitHub's own words for them.
    fn pull_request(approvals: u64, methods: &[&str]) -> serde_json::Value {
        json!({
            "type": "pull_request",
            "parameters": {
                "required_approving_review_count": approvals,
                "allowed_merge_methods": methods,
            },
        })
    }

    /// Feature 274 with issues 1 and 2, one after the other.
    fn serial() -> Dag<u64> {
        Dag::new(vec![Node::new(1, []), Node::new(2, [0])]).unwrap()
    }

    /// Feature 274 with issues 1 and 2, neither waiting on the other.
    fn parallel_work() -> Dag<u64> {
        Dag::new(vec![Node::new(1, []), Node::new(2, [])]).unwrap()
    }

    /// The reading of `fake`'s rules for feature 274 over `work`.
    fn read(fake: &FakeGitHub, work: &Dag<u64>) -> Reading {
        Policy::read(&fake.github(), &FakeGitHub::repo(), FEATURE, work).unwrap()
    }

    #[test]
    fn a_repository_with_no_rulesets_requires_no_checks_and_squashes() {
        let fake = FakeGitHub::spawn();
        let reading = read(&fake, &serial());
        assert_eq!(reading.settled, Ok(Policy::default()));
        assert_eq!(reading.settled.unwrap().merge, Merge::Squash);
        assert!(reading.said.is_empty(), "{:?}", reading.said);
    }

    #[test]
    fn the_rules_are_read_for_the_feature_branch_and_every_issues() {
        let fake = FakeGitHub::spawn();
        read(&fake, &serial());
        assert_eq!(
            fake.requests(),
            ["feature-274", "issue-1", "issue-2"].map(branch_rules),
            "the rules are the whole of what a launch asks GitHub here"
        );
    }

    #[test]
    fn a_ruleset_over_the_feature_branch_yields_its_checks_and_their_apps() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![checks(&[("CI", Some(15368)), ("Docs", None)], false)],
        ));
        let policy = read(&fake, &serial()).settled.unwrap();
        assert_eq!(
            policy.checks,
            [
                Required {
                    context: "CI".to_owned(),
                    app: Some(15368),
                },
                Required {
                    context: "Docs".to_owned(),
                    app: None,
                },
            ]
        );
        assert!(!policy.strict);
    }

    /// A pull-request rule asking for `approvals` reviews, allowing
    /// `methods`, and setting the gate named.
    fn gated(gate: &str) -> serde_json::Value {
        json!({
            "type": "pull_request",
            "parameters": {
                "required_approving_review_count": 0,
                "allowed_merge_methods": ["squash"],
                gate: true,
            },
        })
    }

    /// A merge condition the one who opened the pull request cannot meet
    /// refuses the build, as a required review does: Epik opens every
    /// pull request it merges.
    #[test]
    fn a_pull_request_gate_epik_cannot_meet_refuses_the_build() {
        for gate in [
            "required_review_thread_resolution",
            "require_last_push_approval",
            "require_extra_approval_for_unattributed_changes",
        ] {
            let fake = FakeGitHub::spawn();
            fake.world()
                .rulesets
                .push(Ruleset::new(9, "feature-*", vec![gated(gate)]));
            let words = read(&fake, &serial()).settled.unwrap_err();
            assert!(words.contains(gate), "{words}");
            assert!(words.contains("feature-274"), "{words}");
        }
    }

    /// GitHub flattens every active ruleset, org and repository alike,
    /// into one list of rules, so two rulesets over the same branch send
    /// two rules of a type. Every check either of them requires must
    /// pass before the branch takes a merge, so the policy waits for all
    /// of them.
    #[test]
    fn two_rulesets_requiring_checks_are_waited_for_together() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![checks(&[("org-CI", None)], false)],
        ));
        fake.world().rulesets.push(Ruleset::new(
            10,
            "feature-*",
            vec![checks(&[("repo-CI", None)], false)],
        ));
        let policy = read(&fake, &serial()).settled.unwrap();
        let waited: Vec<_> = policy
            .checks
            .iter()
            .map(|required| required.context.as_str())
            .collect();
        assert_eq!(waited, ["org-CI", "repo-CI"]);
    }

    /// A check two rulesets both require is one check to wait for.
    #[test]
    fn a_check_two_rulesets_both_require_is_waited_for_once() {
        let fake = FakeGitHub::spawn();
        for id in [9, 10] {
            fake.world().rulesets.push(Ruleset::new(
                id,
                "feature-*",
                vec![checks(&[("CI", None)], false)],
            ));
        }
        let policy = read(&fake, &serial()).settled.unwrap();
        assert_eq!(policy.checks.len(), 1, "{:?}", policy.checks);
    }

    /// Strictness is either ruleset's to ask for: GitHub holds the merge
    /// to the stricter of the two.
    #[test]
    fn strictness_asked_by_either_ruleset_is_the_policys() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![checks(&[("lax", None)], false)],
        ));
        fake.world().rulesets.push(Ruleset::new(
            10,
            "feature-*",
            vec![checks(&[("strict", None)], true)],
        ));
        assert!(read(&fake, &serial()).settled.unwrap().strict);
    }

    /// Each ruleset's allowed methods bound the merge, so what the build
    /// may ask for is what all of them allow. Two rulesets whose allowed
    /// methods do not meet leave nothing Epik could ask for, which is a
    /// refusal at launch rather than a merge GitHub answers 405 to.
    #[test]
    fn two_rulesets_allowing_different_methods_leave_only_what_both_allow() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![pull_request(0, &["squash", "rebase"])],
        ));
        fake.world().rulesets.push(Ruleset::new(
            10,
            "feature-*",
            vec![pull_request(0, &["rebase"])],
        ));
        let policy = read(&fake, &serial()).settled.unwrap();
        assert_eq!(policy.merge, Merge::Rebase);
    }

    #[test]
    fn two_rulesets_whose_methods_do_not_meet_refuse_the_build() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![pull_request(0, &["squash"])],
        ));
        fake.world().rulesets.push(Ruleset::new(
            10,
            "feature-*",
            vec![pull_request(0, &["rebase"])],
        ));
        let words = read(&fake, &serial()).settled.unwrap_err();
        assert!(
            words.contains("leave no merge method Epik could use"),
            "{words}"
        );
    }

    #[test]
    fn a_strict_ruleset_over_serial_issues_is_recorded_and_says_nothing() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![checks(&[("CI", None)], true)],
        ));
        let reading = read(&fake, &serial());
        assert!(reading.settled.unwrap().strict);
        assert!(reading.said.is_empty(), "{:?}", reading.said);
    }

    #[test]
    fn a_strict_ruleset_over_parallel_issues_is_said_rather_than_refused() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![checks(&[("CI", None)], true)],
        ));
        let reading = read(&fake, &parallel_work());
        assert!(reading.settled.is_ok(), "a strict policy still builds");
        let said = reading.said.join("\n");
        assert!(said.contains("strict"), "{said}");
        assert!(said.contains("ruleset 9"), "{said}");
    }

    /// What makes a strict rule worth saying is work that is anything
    /// other than a single chain.
    #[test]
    fn issues_run_at_once_where_the_work_is_more_than_one_chain() {
        assert!(!parallel(&Dag::new(vec![]).unwrap()), "no issues at all");
        assert!(!parallel(&Dag::new(vec![Node::new(1, [])]).unwrap()), "one");
        assert!(!parallel(&serial()), "a chain of two");
        assert!(parallel(&parallel_work()), "two with nothing between them");
        let diamond = Dag::new(vec![
            Node::new(1, []),
            Node::new(2, [0]),
            Node::new(3, [0]),
            Node::new(4, [1, 2]),
        ])
        .unwrap();
        assert!(parallel(&diamond), "2 and 3 run at once");
    }

    #[test]
    fn a_ruleset_allowing_only_rebase_yields_rebase() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![pull_request(0, &["rebase"])],
        ));
        let policy = read(&fake, &serial()).settled.unwrap();
        assert_eq!(policy.merge, Merge::Rebase);
    }

    #[test]
    fn a_ruleset_allowing_merge_and_rebase_yields_a_merge_commit() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![pull_request(0, &["merge", "rebase"])],
        ));
        let policy = read(&fake, &serial()).settled.unwrap();
        assert_eq!(policy.merge, Merge::Commit);
    }

    #[test]
    fn a_linear_history_rule_takes_the_merge_commit_out_of_the_methods() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![
                pull_request(0, &["merge", "rebase"]),
                json!({ "type": "required_linear_history" }),
            ],
        ));
        let policy = read(&fake, &serial()).settled.unwrap();
        assert_eq!(policy.merge, Merge::Rebase);
    }

    #[test]
    fn a_required_review_refuses_the_build_naming_the_rule_and_the_ruleset() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            41,
            "feature-*",
            vec![pull_request(1, &["squash"])],
        ));
        let refusal = read(&fake, &serial()).settled.unwrap_err();
        assert!(refusal.contains("approving review"), "{refusal}");
        assert!(refusal.contains("feature-274"), "{refusal}");
        assert!(refusal.contains("ruleset 41"), "{refusal}");
        assert!(refusal.contains("fake/repo"), "{refusal}");
    }

    #[test]
    fn a_merge_queue_refuses_the_build() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![json!({ "type": "merge_queue" })],
        ));
        let refusal = read(&fake, &serial()).settled.unwrap_err();
        assert!(refusal.contains("merge queue"), "{refusal}");
    }

    #[test]
    fn required_signatures_on_an_issue_branch_refuse_the_build() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "issue-*",
            vec![json!({ "type": "required_signatures" })],
        ));
        let refusal = read(&fake, &serial()).settled.unwrap_err();
        assert!(refusal.contains("signed"), "{refusal}");
        assert!(refusal.contains("issue-1"), "{refusal}");
    }

    #[test]
    fn a_creation_or_update_restriction_on_either_branch_refuses_the_build() {
        for (pattern, branch, rule) in [
            ("feature-*", "feature-274", "creation"),
            ("issue-*", "issue-1", "update"),
        ] {
            let fake = FakeGitHub::spawn();
            fake.world()
                .rulesets
                .push(Ruleset::new(9, pattern, vec![json!({ "type": rule })]));
            let refusal = read(&fake, &serial()).settled.unwrap_err();
            assert!(refusal.contains(branch), "{refusal}");
            assert!(refusal.contains("ruleset 9"), "{refusal}");
        }
    }

    #[test]
    fn every_refusal_is_named_at_once_rather_than_the_first_alone() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![
                pull_request(2, &["squash"]),
                json!({ "type": "merge_queue" }),
            ],
        ));
        let refusal = read(&fake, &serial()).settled.unwrap_err();
        assert!(refusal.contains("approving review"), "{refusal}");
        assert!(refusal.contains("merge queue"), "{refusal}");
    }

    #[test]
    fn a_rule_type_the_policy_does_not_know_is_said_and_changes_nothing() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "feature-*",
            vec![json!({ "type": "quantum_entanglement" })],
        ));
        let reading = read(&fake, &serial());
        assert_eq!(
            reading.settled,
            Ok(Policy::default()),
            "an unknown rule leaves the policy where it stood"
        );
        let said = reading.said.join("\n");
        assert!(said.contains("quantum_entanglement"), "{said}");
        assert!(said.contains("ruleset 9"), "{said}");
    }

    /// Every rule the policy does not know is said, whichever branch it
    /// stands over, and the build goes on.
    #[test]
    fn an_unknown_rule_on_an_issue_branch_is_said_and_changes_nothing() {
        let fake = FakeGitHub::spawn();
        fake.world().rulesets.push(Ruleset::new(
            9,
            "issue-*",
            vec![json!({ "type": "non_fast_forward" })],
        ));
        let reading = read(&fake, &serial());
        assert_eq!(reading.settled, Ok(Policy::default()));
        let said = reading.said.join("\n");
        assert!(said.contains("non_fast_forward"), "{said}");
        assert!(said.contains("issue-1"), "{said}");
        assert!(
            said.lines().count() == 2,
            "one line per branch it stands over: {said}"
        );
    }
}
