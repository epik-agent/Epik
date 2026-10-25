import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import { readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { gh, ghRaw, graphql, MAX_PROCESSES } from '../src/gh.js';
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

test('graphql sends each variable as its own field, integers as -F and strings as -f', async () => {
  restore = await fakeGh(`printf '{"data": {"args": ['; for a in "$@"; do printf '"%s",' "$a"; done; printf '""]}}'`);
  const { args } = await graphql('query($owner: String!, $number: Int!) { x }', { owner: 'epik-agent', number: 12, cursor: undefined });
  assert.deepEqual(args, [
    'api', 'graphql', '-f', 'query=query($owner: String!, $number: Int!) { x }', '-f', 'owner=epik-agent', '-F', 'number=12', '',
  ]);
});

test('gh printing more than 64 MB is a GhError naming the limit', async () => {
  restore = await fakeGh(`'${process.execPath}' -e 'process.stdout.write(Buffer.alloc(64 * 1024 * 1024 + 1))'`);
  await assert.rejects(gh(['run', 'view', '1', '--log']), {
    name: 'GhError',
    message: 'gh printed more than 64 MB: gh run view 1 --log',
  });
});

test('gh stopped by a signal is a GhError naming the signal', async () => {
  restore = await fakeGh(`kill -TERM $$`);
  await assert.rejects(gh(['issue', 'list']), { name: 'GhError', message: 'gh was stopped by SIGTERM: gh issue list' });
});

test('ghRaw reports gh stopped by a signal as a value', async () => {
  restore = await fakeGh(`kill -TERM $$`);
  assert.deepEqual(await ghRaw(['issue', 'list']), {
    ok: false,
    exit_code: -1,
    stdout: '',
    stderr: 'gh was stopped by SIGTERM: gh issue list',
  });
});

test(`at most ${MAX_PROCESSES} gh processes run at once`, async () => {
  const log = join(tmpdir(), `gh-concurrency-${process.pid}.log`);
  restore = await fakeGh(`echo + >> '${log}'; '${process.execPath}' -e 'setTimeout(() => {}, 100)'; echo - >> '${log}'`);
  try {
    await Promise.all(Array.from({ length: 3 * MAX_PROCESSES }, () => gh(['api', 'x'])));
    let running = 0;
    let most = 0;
    for (const mark of (await readFile(log, 'utf8')).trim().split('\n')) {
      running += mark === '+' ? 1 : -1;
      most = Math.max(most, running);
    }
    assert.equal(most, MAX_PROCESSES);
  } finally {
    await rm(log, { force: true });
  }
});
