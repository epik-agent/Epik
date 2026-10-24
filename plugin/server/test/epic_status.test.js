import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import epicStatus, { ci, linkedPr } from '../src/tools/epic_status.js';
import { fakeGitHub, pages } from './fake-github.js';

let restore;
afterEach(() => restore?.());

const repository = (repo) => ({ full_name: repo });
const issue = (number, title, state = 'open', blockedBy = 0) => ({
  number,
  title,
  state,
  repository_url: 'https://api.github.com/repos/o/r',
  issue_dependencies_summary: { total_blocked_by: blockedBy },
});
const reference = (number, { state = 'closed', merged_at = null, body = '', repo = 'o/r', pr = true } = {}) => ({
  event: 'cross-referenced',
  source: {
    type: 'issue',
    issue: {
      number,
      title: `PR ${number}`,
      body,
      state,
      html_url: `https://github.com/${repo}/pull/${number}`,
      repository: repository(repo),
      ...(pr && { pull_request: { merged_at } }),
    },
  },
});

test('the linked PR is the open one among those referencing the issue', () => {
  const timeline = [
    reference(16, { merged_at: '2026-06-28T21:44:46Z' }),
    reference(17, { state: 'open' }),
    { event: 'labeled' },
  ];
  assert.deepEqual(linkedPr(timeline, 'o/r', 10), { number: 17, state: 'open', url: 'https://github.com/o/r/pull/17' });
});

test('with none open, the linked PR is the most recently merged', () => {
  const timeline = [
    reference(16, { merged_at: '2026-06-28T21:44:46Z' }),
    reference(19, { merged_at: '2026-06-29T00:00:00Z' }),
    reference(18),
  ];
  assert.deepEqual(linkedPr(timeline, 'o/r', 10), {
    number: 19,
    state: 'merged',
    url: 'https://github.com/o/r/pull/19',
  });
});

test('a PR that closes the issue outranks one that only mentions it', () => {
  const timeline = [
    reference(16, { merged_at: '2026-06-28T21:44:46Z', body: 'Closes #10.' }),
    reference(22, { merged_at: '2026-06-28T22:31:35Z', body: 'Rolls up #10 and #11.' }),
    reference(23, { state: 'open', body: 'Fixes #100.' }),
  ];
  assert.equal(linkedPr(timeline, 'o/r', 10).number, 16);
});

test('issues, and PRs in other repositories, are never the linked PR', () => {
  const timeline = [reference(12, { pr: false, state: 'open' }), reference(5, { state: 'open', repo: 'x/y' })];
  assert.equal(linkedPr(timeline, 'o/r', 10), null);
});

test('ci combines the check runs on a commit', () => {
  const run = (status, conclusion = null) => ({ status, conclusion });
  assert.equal(ci([run('completed', 'success'), run('completed', 'skipped'), run('completed', 'neutral')]), 'success');
  assert.equal(ci([run('completed', 'success'), run('in_progress'), run('queued')]), 'pending');
  assert.equal(ci([run('in_progress'), run('completed', 'failure')]), 'failure');
  assert.equal(ci([run('completed', 'cancelled')]), 'failure');
  assert.equal(ci([]), null);
});

test('epic_status reports each sub-issue with its PR, CI, in-epic blockers and level', async () => {
  restore = await fakeGitHub({
    [pages('repos/o/r/issues/9/sub_issues')]: [
      [issue(10, 'Ten', 'closed'), issue(11, 'Eleven', 'open', 2), issue(12, 'Twelve', 'open', 1)],
    ],
    [pages('repos/o/r/issues/11/dependencies/blocked_by')]: [[issue(10, 'Ten'), issue(99, 'Elsewhere')]],
    [pages('repos/o/r/issues/12/dependencies/blocked_by')]: [[issue(11, 'Eleven')]],
    [pages('repos/o/r/issues/10/timeline')]: [[reference(16, { merged_at: '2026-06-28T21:44:46Z' })]],
    [pages('repos/o/r/issues/11/timeline')]: [[reference(20, { state: 'open' })]],
    [pages('repos/o/r/issues/12/timeline')]: [[]],
    [pages('repos/o/r/commits/refs/pull/16/head/check-runs')]: [
      { check_runs: [{ status: 'completed', conclusion: 'success' }] },
    ],
    [pages('repos/o/r/commits/refs/pull/20/head/check-runs')]: [
      { check_runs: [{ status: 'queued', conclusion: null }] },
    ],
  });
  assert.deepEqual(await epicStatus.run({ repo: 'o/r', epic_issue_number: 9 }), {
    epic: 9,
    repo: 'o/r',
    issues: [
      {
        number: 10,
        title: 'Ten',
        state: 'closed',
        linked_pr: { number: 16, state: 'merged', url: 'https://github.com/o/r/pull/16' },
        ci: 'success',
        blocked_by: [],
        position: 0,
      },
      {
        number: 11,
        title: 'Eleven',
        state: 'open',
        linked_pr: { number: 20, state: 'open', url: 'https://github.com/o/r/pull/20' },
        ci: 'pending',
        blocked_by: [10],
        position: 1,
      },
      { number: 12, title: 'Twelve', state: 'open', linked_pr: null, ci: null, blocked_by: [11], position: 2 },
    ],
    cycles: [],
  });
});

test('epic_status levels around a cycle and lists it', async () => {
  restore = await fakeGitHub({
    [pages('repos/o/r/issues/9/sub_issues')]: [
      [issue(1, 'One', 'open', 1), issue(2, 'Two', 'open', 1), issue(3, 'Three')],
    ],
    [pages('repos/o/r/issues/1/dependencies/blocked_by')]: [[issue(2, 'Two')]],
    [pages('repos/o/r/issues/2/dependencies/blocked_by')]: [[issue(1, 'One')]],
    [pages('repos/o/r/issues/1/timeline')]: [[]],
    [pages('repos/o/r/issues/2/timeline')]: [[]],
    [pages('repos/o/r/issues/3/timeline')]: [[]],
  });
  const { issues, cycles } = await epicStatus.run({ repo: 'o/r', epic_issue_number: 9 });
  assert.deepEqual(
    issues.map(({ number, position }) => [number, position]),
    [
      [1, null],
      [2, null],
      [3, 0],
    ],
  );
  assert.deepEqual(cycles, [[1, 2]]);
});
