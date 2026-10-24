import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import ghRaw from '../src/tools/gh_raw.js';
import { fakeGh, noGh } from './fake-gh.js';

let restore;
afterEach(() => restore());

test('gh_raw returns the outcome of a successful command', async () => {
  restore = await fakeGh(`printf '%s ' "$@"`);
  assert.deepEqual(await ghRaw.run({ args: ['release', 'list', '--repo', 'epik-agent/Epik'] }), {
    ok: true,
    exit_code: 0,
    stdout: 'release list --repo epik-agent/Epik ',
    stderr: '',
  });
});

test('gh_raw returns the outcome of a failing command', async () => {
  restore = await fakeGh(`echo 'unknown command "nope"' >&2; exit 1`);
  assert.deepEqual(await ghRaw.run({ args: ['nope'] }), {
    ok: false,
    exit_code: 1,
    stdout: '',
    stderr: 'unknown command "nope"\n',
  });
});

test('gh_raw reports gh missing as a failed command', async () => {
  restore = await noGh();
  assert.deepEqual(await ghRaw.run({ args: ['release', 'list'] }), {
    ok: false,
    exit_code: -1,
    stdout: '',
    stderr: 'gh is not installed. Install it from https://cli.github.com/',
  });
});
