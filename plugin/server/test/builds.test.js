import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, test } from 'node:test';

import buildList from '../src/tools/build_list.js';
import buildStatus from '../src/tools/build_status.js';
import { fakeApp } from './fake-app.js';
import { emptyPath } from './fake-gh.js';

let dir;
let app;
let restorePath;
const saved = {};

beforeEach(async () => {
  dir = await mkdtemp(join(tmpdir(), 'builds-'));
  saved.EPIK_SOCKET = process.env.EPIK_SOCKET;
  process.env.EPIK_SOCKET = join(dir, 'epik.sock');
  // Nothing on the PATH, so no real epik-app could be started.
  restorePath = await emptyPath();
});

afterEach(async () => {
  await app?.close();
  app = undefined;
  await restorePath();
  for (const [name, value] of Object.entries(saved)) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  await rm(dir, { recursive: true, force: true });
});

const run = {
  run_id: 'o-r-356-20261028T212859Z',
  repo: 'o/r',
  number: 356,
  title: 'The Epik app runs builds',
  kind: 'epic',
  branch: 'daemon',
  status: 'running',
  reason: null,
  pull_request: null,
  members: [
    {
      number: 357,
      title: 'Rust workspace and CI',
      blocked_by: [],
      status: 'merged',
      reason: null,
      pull_request: 367,
      duration_ms: 512000,
      cost_usd: 1.12,
    },
  ],
  started_at: '2026-10-28T21:28:59Z',
  ended_at: null,
  elapsed_ms: 600000,
  cost_usd: 1.12,
};

test("build_status asks the app for a run's status and returns the run's state", async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, () => ({ type: 'status', ...run }));
  assert.deepEqual(await buildStatus.run({ run_id: run.run_id }), run);
  assert.deepEqual(app.requests, [{ type: 'status', run_id: run.run_id }]);
});

test("build_status of an unknown run is the app's error", async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, () => ({ type: 'error', message: 'no run o-r-1-x' }));
  await assert.rejects(buildStatus.run({ run_id: 'o-r-1-x' }), { message: 'no run o-r-1-x' });
});

test('build_list asks the app for its latest runs and returns them', async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, () => ({ type: 'runs', runs: [run] }));
  assert.deepEqual(await buildList.run({ limit: 5 }), [run]);
  assert.deepEqual(app.requests, [{ type: 'list', limit: 5 }]);
});

test("build_list without a limit leaves the default to the app", async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, () => ({ type: 'runs', runs: [] }));
  assert.deepEqual(await buildList.run({}), []);
  assert.deepEqual(app.requests, [{ type: 'list' }]);
});

test("when the app isn't running, neither tool starts it, and each says it isn't running", async () => {
  await assert.rejects(buildStatus.run({ run_id: run.run_id }), { message: /Epik app isn't running/ });
  await assert.rejects(buildList.run({}), { message: /Epik app isn't running/ });
});
