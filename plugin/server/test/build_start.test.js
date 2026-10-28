import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, test } from 'node:test';

import buildStart from '../src/tools/build_start.js';
import { fakeApp, started } from './fake-app.js';

let dir;
let app;
const saved = {};

beforeEach(async () => {
  dir = await mkdtemp(join(tmpdir(), 'build-start-'));
  for (const name of ['EPIK_SOCKET', 'CLAUDE_PLUGIN_ROOT']) saved[name] = process.env[name];
  process.env.EPIK_SOCKET = join(dir, 'epik.sock');
  process.env.CLAUDE_PLUGIN_ROOT = '/plugins/epik';
});

afterEach(async () => {
  await app?.close();
  app = undefined;
  for (const [name, value] of Object.entries(saved)) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  await rm(dir, { recursive: true, force: true });
});

test('a build is asked of the Epik app, and its run is returned with its events log', async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, started);
  const result = await buildStart.run({ repo: 'o/r', number: 7 });

  assert.deepEqual(app.requests, [{ type: 'build', repo: 'o/r', number: 7, plugin_dir: '/plugins/epik' }]);
  assert.deepEqual(result, {
    run_id: 'o-r-7-20261028T212859Z',
    kind: 'issue',
    branch: 'main',
    run_dir: '/home/me/.epik/runs/o-r-7-20261028T212859Z',
    events: join('/home/me/.epik/runs/o-r-7-20261028T212859Z', 'events.jsonl'),
  });
});

test('a given branch is sent to the app', async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, started);
  const result = await buildStart.run({ repo: 'o/r', number: 7, branch: 'epic/9-x' });
  assert.equal(app.requests[0].branch, 'epic/9-x');
  assert.equal(result.branch, 'epic/9-x');
});

test("the app's refusal is the tool's error", async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, () => ({ type: 'error', message: "couldn't build o/r#7: it is closed" }));
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), { message: "couldn't build o/r#7: it is closed" });
});

test('without CLAUDE_PLUGIN_ROOT nothing is built', async () => {
  delete process.env.CLAUDE_PLUGIN_ROOT;
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), { message: /CLAUDE_PLUGIN_ROOT is not set/ });
});
