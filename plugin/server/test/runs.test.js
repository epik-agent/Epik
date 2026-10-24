import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import runGet from '../src/tools/run_get.js';
import runList from '../src/tools/run_list.js';
import runLogs from '../src/tools/run_logs.js';
import { expectGh, fakeGh } from './fake-gh.js';

let restore;
afterEach(() => restore());

const LIST_FIELDS = 'databaseId,name,workflowName,headBranch,status,conclusion,url';
const GET_FIELDS = `${LIST_FIELDS},event,headSha,createdAt,updatedAt`;
const run = { databaseId: 42, name: 'CI', status: 'completed', conclusion: 'success' };

test('run_list lists the latest 20 runs by default', async () => {
  restore = await expectGh(
    ['run', 'list', '--repo', 'o/r', '--limit', '20', '--json', LIST_FIELDS],
    JSON.stringify([run]),
  );
  assert.deepEqual(await runList.run({ repo: 'o/r' }), [run]);
});

test('run_list filters by workflow, branch and status', async () => {
  restore = await expectGh(
    [
      'run', 'list', '--repo', 'o/r', '--limit', '5', '--workflow', 'ci.yml',
      '--branch', 'main', '--status', 'failure', '--json', LIST_FIELDS,
    ],
    '[]',
  );
  assert.deepEqual(
    await runList.run({ repo: 'o/r', workflow: 'ci.yml', branch: 'main', status: 'failure', limit: 5 }),
    [],
  );
});

test('run_list fails when there is no such workflow', async () => {
  restore = await fakeGh(`echo 'could not find any workflows named nope' >&2; exit 1`);
  await assert.rejects(runList.run({ repo: 'o/r', workflow: 'nope' }), { name: 'GhError', message: /nope/ });
});

test('run_get returns one run in detail', async () => {
  restore = await expectGh(['run', 'view', '42', '--repo', 'o/r', '--json', GET_FIELDS], JSON.stringify(run));
  assert.deepEqual(await runGet.run({ repo: 'o/r', run_id: 42 }), run);
});

test('run_get fails when there is no such run', async () => {
  restore = await fakeGh(`echo 'HTTP 404: Not Found' >&2; exit 1`);
  await assert.rejects(runGet.run({ repo: 'o/r', run_id: 1 }), { name: 'GhError', message: /404/ });
});

test("run_logs returns the run's logs", async () => {
  restore = await expectGh(['run', 'view', '42', '--repo', 'o/r', '--log'], 'build\tok\n');
  assert.equal(await runLogs.run({ repo: 'o/r', run_id: 42 }), 'build\tok\n');
});

test("run_logs returns one job's logs", async () => {
  restore = await expectGh(['run', 'view', '42', '--repo', 'o/r', '--job', '7', '--log'], 'job 7\n');
  assert.equal(await runLogs.run({ repo: 'o/r', run_id: 42, job_id: 7 }), 'job 7\n');
});

test("run_logs returns only the failed steps' logs", async () => {
  restore = await expectGh(['run', 'view', '42', '--repo', 'o/r', '--log-failed'], 'failed\n');
  assert.equal(await runLogs.run({ repo: 'o/r', run_id: 42, failed_only: true }), 'failed\n');
});

test('run_logs fails while the run is in progress', async () => {
  restore = await fakeGh(`echo 'run 42 is still in progress; logs will be available when it is complete' >&2; exit 1`);
  await assert.rejects(runLogs.run({ repo: 'o/r', run_id: 42 }), { name: 'GhError', message: /in progress/ });
});
