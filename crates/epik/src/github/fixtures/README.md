# Captured GitHub payloads

Real answers from the real API about this very repository, captured
2026-08-05 and decoded by the unit tier without a network. The REST ones were
read unauthenticated — exactly what the CI smoke sees — and the GraphQL ones
through a locally authenticated `gh`, because GraphQL answers nothing without
a token.

| File | Provenance |
| --- | --- |
| `repo.json` | `GET /repos/epik-agent/Epik` — the default branch lives here |
| `issue.json` | `GET /repos/epik-agent/Epik/issues/106` — the issue that specified this module |
| `pull.json` | `GET /repos/epik-agent/Epik/pulls/91` — a merged PR, so `merged`, `head`, and `base` are all exercised |
| `check_runs.json` | `GET /repos/epik-agent/Epik/commits/main/check-runs` — five completed runs, all `success` |
| `graph_parent.json` | GraphQL: the graph query over #102, whose `subIssues` were #105 and #106 |
| `graph_blocked.json` | GraphQL: the same query over #106, then blocked by #105 |

What the captures are here to catch:

- An issue's `body` is a string in these files but `null` on GitHub when
  empty; the null case is covered by an inline payload in the tests.
- REST spells state `open`; GraphQL shouts `OPEN`. One fixture of each keeps
  both spellings decoding into the same `State`.
- Every payload carries dozens of fields the client never modelled
  (`node_id`, `_links`, reactions, the works). None of it may break decoding.

To recapture the REST ones:

```sh
curl -s https://api.github.com/repos/epik-agent/Epik > repo.json
curl -s https://api.github.com/repos/epik-agent/Epik/issues/106 > issue.json
curl -s https://api.github.com/repos/epik-agent/Epik/pulls/91 > pull.json
curl -s https://api.github.com/repos/epik-agent/Epik/commits/main/check-runs > check_runs.json
```

And the GraphQL ones (`-F number=102` for the parent, `106` for the blocked):

```sh
gh api graphql -f query='query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) { issue(number: $number) {
    number title state body
    subIssues(first: 100) { nodes { number title state } }
    blockedBy(first: 100) { nodes { number title state } } } } }' \
  -F owner=epik-agent -F name=Epik -F number=102 > graph_parent.json
```

Recapturing rewrites history: the live repository has moved on since these
were taken, so expect the assertions in `github.rs` to need re-reading, not
just re-running.
