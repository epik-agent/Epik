import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import listBlockedBy from '../src/tools/issue_list_blocked_by.js';
import removeBlockedBy from '../src/tools/issue_remove_blocked_by.js';
import setBlockedBy from '../src/tools/issue_set_blocked_by.js';
import { calls, fakeGitHub, pages } from './fake-github.js';

let restore;
afterEach(() => restore());

const edges = 'repos/o/r/issues/12/dependencies/blocked_by';

test('setting an edge looks up the blocking issue ID, then posts it', async () => {
  restore = await fakeGitHub({
    'api repos/o/r/issues/10': { id: 1010, number: 10 },
    [`api --method POST ${edges} -F issue_id=1010`]: { number: 12 },
  });
  assert.deepEqual(await setBlockedBy.run({ repo: 'o/r', issue_number: 12, blocked_by_number: 10 }), {
    repo: 'o/r',
    issue_number: 12,
    blocked_by: { repo: 'o/r', number: 10 },
  });
  assert.deepEqual(await calls(), ['api repos/o/r/issues/10', `api --method POST ${edges} -F issue_id=1010`]);
});

test('a blocking issue in another repository is looked up there', async () => {
  restore = await fakeGitHub({
    'api repos/x/y/issues/3': { id: 33, number: 3 },
    [`api --method POST ${edges} -F issue_id=33`]: { number: 12 },
  });
  const result = await setBlockedBy.run({
    repo: 'o/r',
    issue_number: 12,
    blocked_by_number: 3,
    blocked_by_repo: 'x/y',
  });
  assert.deepEqual(result.blocked_by, { repo: 'x/y', number: 3 });
});

test('removing an edge looks up the blocking issue ID, then deletes it', async () => {
  restore = await fakeGitHub({
    'api repos/o/r/issues/10': { id: 1010, number: 10 },
    [`api --method DELETE ${edges}/1010`]: { number: 12 },
  });
  assert.deepEqual(await removeBlockedBy.run({ repo: 'o/r', issue_number: 12, blocked_by_number: 10 }), {
    repo: 'o/r',
    issue_number: 12,
    removed_blocked_by: { repo: 'o/r', number: 10 },
  });
  assert.deepEqual(await calls(), ['api repos/o/r/issues/10', `api --method DELETE ${edges}/1010`]);
});

test('setting an edge on a missing issue fails with GitHub’s message', async () => {
  restore = await fakeGitHub({});
  await assert.rejects(setBlockedBy.run({ repo: 'o/r', issue_number: 12, blocked_by_number: 99 }), {
    name: 'GhError',
    message: /HTTP 404: Not Found/,
  });
});

test('listing gives every blocking issue, across all pages', async () => {
  restore = await fakeGitHub({
    [pages(edges)]: [
      [{ number: 10, title: 'Ten', state: 'open', repository_url: 'https://api.github.com/repos/o/r', id: 1 }],
      [{ number: 3, title: 'Three', state: 'closed', repository_url: 'https://api.github.com/repos/x/y', id: 2 }],
    ],
  });
  assert.deepEqual(await listBlockedBy.run({ repo: 'o/r', issue_number: 12 }), [
    { number: 10, title: 'Ten', state: 'open', repository: 'o/r' },
    { number: 3, title: 'Three', state: 'closed', repository: 'x/y' },
  ]);
});
