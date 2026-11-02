---
name: review
description: Review a pull request against its issue and Epik's rules, and post one GitHub review with a verdict
argument-hint: [pull request number or GitHub URL] [round]
disable-model-invocation: true
---

Review the pull request by following "Reviewing a pull request", the first part of the procedure in
`${CLAUDE_SKILL_DIR}/procedure.md`. Epik's rules are in `${CLAUDE_SKILL_DIR}/../assistant/rules.md`. The issue is the one
the pull request's body closes (`Closes #<issue number>`); its sub-issues, if it is an epic, are
`gh api repos/<o>/<r>/issues/<issue>/sub_issues`. The round is 1 unless one is given.
