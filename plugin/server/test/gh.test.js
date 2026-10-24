import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import { gh } from '../src/gh.js';
import { fakeGh, noGh } from './fake-gh.js';

let restore;
afterEach(() => restore());

test('gh returns stdout and passes each argument through unsplit', async () => {
  restore = await fakeGh(`printf '%s\\n' "$@"`);
  assert.equal(await gh(['issue', 'view', 'a title; rm -rf /']), 'issue\nview\na title; rm -rf /\n');
});

test('gh parses stdout when asked for JSON', async () => {
  restore = await fakeGh(`echo '{"number": 309, "labels": ["epic"]}'`);
  assert.deepEqual(await gh(['issue', 'view', '309', '--json', 'number,labels'], { json: true }), {
    number: 309,
    labels: ['epic'],
  });
});

test('a failing gh call reports its stderr and exit code', async () => {
  restore = await fakeGh(`echo 'GraphQL: Could not resolve to an Issue' >&2; exit 2`);
  await assert.rejects(gh(['issue', 'view', '999']), {
    name: 'GhError',
    message: 'GraphQL: Could not resolve to an Issue\n(exit code 2)',
  });
});

test('gh signed out names gh auth login as the fix', async () => {
  restore = await fakeGh(`echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4`);
  await assert.rejects(gh(['issue', 'list']), {
    message: /^gh is not signed in\. Run `gh auth login`\.\n.*please run: {2}gh auth login\n\(exit code 4\)$/,
  });
});

test('gh missing names installing it as the fix', async () => {
  restore = await noGh();
  await assert.rejects(gh(['issue', 'list']), {
    name: 'GhError',
    message: 'gh is not installed. Install it from https://cli.github.com/',
  });
});
