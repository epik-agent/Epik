import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import labelCreate from '../src/tools/label_create.js';
import labelDelete from '../src/tools/label_delete.js';
import labelList from '../src/tools/label_list.js';
import { expectGh, fakeGh } from './fake-gh.js';

let restore;
afterEach(() => restore());

const epic = { name: 'epic', color: '5319e7', description: 'An issue with sub-issues' };

test('label_list returns every label with its color and description', async () => {
  restore = await expectGh(
    ['label', 'list', '--repo', 'o/r', '--json', 'name,color,description'],
    JSON.stringify([epic]),
  );
  assert.deepEqual(await labelList.run({ repo: 'o/r' }), [epic]);
});

test('label_list fails when there is no such repository', async () => {
  restore = await fakeGh(`echo 'HTTP 404: Not Found' >&2; exit 1`);
  await assert.rejects(labelList.run({ repo: 'o/nope' }), { name: 'GhError', message: /404/ });
});

test('label_create creates a label and returns it', async () => {
  restore = await expectGh(['label', 'create', 'epic', '--repo', 'o/r', '--color', '5319e7']);
  assert.deepEqual(await labelCreate.run({ repo: 'o/r', name: 'epic', color: '5319e7' }), {
    name: 'epic',
    color: '5319e7',
    description: '',
  });
});

test('label_create with force updates an existing label', async () => {
  restore = await expectGh([
    'label', 'create', 'epic', '--repo', 'o/r', '--color', '5319e7',
    '--description', 'An issue with sub-issues', '--force',
  ]);
  assert.deepEqual(await labelCreate.run({ repo: 'o/r', ...epic, force: true }), epic);
});

test('label_create fails when the label exists', async () => {
  restore = await fakeGh(`echo 'label with name "epic" already exists; use --force to update' >&2; exit 1`);
  await assert.rejects(labelCreate.run({ repo: 'o/r', name: 'epic', color: '5319e7' }), {
    name: 'GhError',
    message: /already exists/,
  });
});

test('label_delete deletes the label', async () => {
  restore = await expectGh(['label', 'delete', 'epic', '--repo', 'o/r', '--yes']);
  assert.deepEqual(await labelDelete.run({ repo: 'o/r', name: 'epic' }), { deleted: 'epic', repo: 'o/r' });
});

test('label_delete fails when there is no such label', async () => {
  restore = await fakeGh(`echo 'HTTP 404: Not Found' >&2; exit 1`);
  await assert.rejects(labelDelete.run({ repo: 'o/r', name: 'nope' }), { name: 'GhError', message: /404/ });
});
