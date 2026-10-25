import * as z from 'zod';

import { blockers } from '../blocked_by.js';
import { levels } from '../graph.js';
import { byUrl, issueUrl, pullUrl, repoOf, sameRepo } from '../ref.js';
import { list } from '../rest.js';

// The pull request linked to issue `number` in `repo`, given the issue's
// timeline: among the pull requests in `repo` that cross-reference it, open
// and merged ones rank above closed ones; then those whose title or body
// closes the issue; then an open one; then the most recently merged. Null
// when none does.
export function linkedPr(timeline, repo, number) {
  const escaped = repo.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const closing = new RegExp(`\\b(close[sd]?|fix(e[sd])?|resolve[sd]?):?\\s+(${escaped})?#${number}\\b`, 'i');
  const rank = (pr) => [
    pr.state === 'closed' && !pr.pull_request.merged_at ? 1 : 0,
    closing.test(`${pr.title}\n${pr.body ?? ''}`) ? 0 : 1,
    pr.state === 'open' ? 0 : 1,
    -Date.parse(pr.pull_request.merged_at) || 0,
  ];
  const [pr] = timeline
    .filter((event) => event.event === 'cross-referenced')
    .map((event) => event.source.issue)
    .filter((source) => source.pull_request && sameRepo(source.repository.full_name, repo))
    .map((source) => [rank(source), source])
    .sort(([a], [b]) => a.map((x, i) => x - b[i]).find(Boolean) ?? 0)
    .map(([, source]) => source);
  if (!pr) return null;
  const state = pr.pull_request.merged_at ? 'merged' : pr.state;
  return { number: pr.number, state, url: pullUrl(repo, pr.number) };
}

// The combined result of `checkRuns`: failure if any failed, pending if any
// is still running, success if all passed, null with no runs.
export function ci(checkRuns) {
  if (checkRuns.length === 0) return null;
  const passed = ['success', 'neutral', 'skipped'];
  const done = checkRuns.filter((run) => run.status === 'completed');
  if (done.some((run) => !passed.includes(run.conclusion))) return 'failure';
  return done.length < checkRuns.length ? 'pending' : 'success';
}

// The CI result on pull request `pr`'s head commit.
const prCi = async (repo, pr) =>
  ci(await list(`repos/${repo}/commits/refs/pull/${pr.number}/head/check-runs`, 'check_runs'));

// Resolves to `promise`'s value or, when it fails, to `fallback`, recording
// the failure in `errors`.
const attempt = (promise, fallback, errors) =>
  promise.catch((error) => {
    errors.push(error.message);
    return fallback;
  });

// The status of sub-issue `issue`, whose blockers count only when in `epic`,
// the URLs of the epic's sub-issues.
async function row(issue, epic) {
  const repo = repoOf(issue);
  const errors = [];
  const [timeline, blocking] = await Promise.all([
    attempt(list(`repos/${repo}/issues/${issue.number}/timeline`), [], errors),
    issue.issue_dependencies_summary.total_blocked_by ? attempt(blockers(repo, issue.number), [], errors) : [],
  ]);
  const pr = linkedPr(timeline, repo, issue.number);
  return {
    url: issueUrl(repo, issue.number),
    number: issue.number,
    title: issue.title,
    state: issue.state,
    linked_pr: pr,
    ci: pr && (await attempt(prCi(repo, pr), null, errors)),
    blocked_by: blocking.map((blocker) => issueUrl(repoOf(blocker), blocker.number)).filter((url) => epic.has(url)),
    ...(errors.length > 0 && { degraded: true, errors }),
  };
}

export default {
  name: 'epic_status',
  description: `Report the status of each sub-issue of an epic.

Issues and pull requests are named by URL, which holds their repository as
well as their number; sub-issues may be in other repositories.

For each sub-issue: url, number, title, state; linked_pr, the pull request in
its repository that references it (number, state, url); ci, the combined
check-run result on that pull request's head commit (failure, pending,
success, or null); blocked_by, the URLs of its blocking issues that are
sub-issues of the epic; and position, its level in the dependency graph (0
with no blockers, else one more than its deepest blocker). Issues on a
dependency cycle have position null, and each cycle is listed under the
top-level cycles key. When a read fails, its sub-issue has degraded: true and
errors, the failures, and the fields the read would fill are null or empty.

Args:
    repo: Repository in owner/name format.
    epic_issue_number: The epic issue.`,
  inputSchema: { repo: z.string(), epic_issue_number: z.number().int() },
  run: async ({ repo, epic_issue_number }) => {
    const subIssues = await list(`repos/${repo}/issues/${epic_issue_number}/sub_issues`);
    const epic = new Set(subIssues.map((issue) => issueUrl(repoOf(issue), issue.number)));
    const rows = await Promise.all(subIssues.map((issue) => row(issue, epic)));
    const { positions, cycles } = levels(new Map(rows.map((r) => [r.url, r.blocked_by])), byUrl);
    return {
      epic: issueUrl(repo, epic_issue_number),
      issues: rows.map((r) => ({ ...r, position: positions.get(r.url) })),
      cycles,
    };
  },
};
