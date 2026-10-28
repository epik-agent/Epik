import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeEach, test } from 'node:test';

import { request, socketPath } from '../src/app.js';
import { fakeApp } from './fake-app.js';
import { emptyPath, fakeProgram } from './fake-gh.js';

const fakeEpikApp = fileURLToPath(new URL('./fake-epik-app.js', import.meta.url));

let dir;
let app;
let restorePath;
const saved = {};

beforeEach(async () => {
  dir = await mkdtemp(join(tmpdir(), 'app-'));
  for (const name of ['EPIK_SOCKET', 'FAKE_APP_ARGS']) saved[name] = process.env[name];
  process.env.EPIK_SOCKET = join(dir, 'epik.sock');
  // Nothing on the PATH, so no real epik-app is started.
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

// Puts on the PATH, in place of the empty one, an epik-app that runs the
// shell commands `script`.
async function installEpikApp(script) {
  await restorePath();
  restorePath = await fakeProgram('epik-app', script);
}

test('the socket is ~/.epik/epik.sock unless EPIK_SOCKET overrides it', () => {
  assert.equal(socketPath(), join(dir, 'epik.sock'));
  delete process.env.EPIK_SOCKET;
  const expected = process.platform === 'win32' ? '\\\\.\\pipe\\epik' : join(process.env.HOME, '.epik', 'epik.sock');
  assert.equal(socketPath(), expected);
});

test('a request is one JSON line, and its reply is the line the app sends back', async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, () => ({ type: 'pong', version: '0.1.0' }));
  assert.deepEqual(await request({ type: 'ping' }), { type: 'pong', version: '0.1.0' });
  assert.deepEqual(app.requests, [{ type: 'ping' }]);
});

test('an error reply is thrown with its message', async () => {
  app = await fakeApp(process.env.EPIK_SOCKET, () => ({ type: 'error', message: "couldn't build o/r#8: it is closed" }));
  await assert.rejects(request({ type: 'build' }), { message: "couldn't build o/r#8: it is closed" });
});

test('when nothing listens, epik-app is started with no arguments and the request is sent to it', async () => {
  await installEpikApp(`exec '${process.execPath}' '${fakeEpikApp}' "$@"`);
  process.env.FAKE_APP_ARGS = join(dir, 'args.json');

  const reply = await request({ type: 'build', repo: 'o/r', number: 7 });
  assert.equal(reply.type, 'started');
  assert.equal(reply.run_id, 'o-r-7-20261028T212859Z');
  assert.deepEqual(JSON.parse(await readFile(process.env.FAKE_APP_ARGS, 'utf8')), []);
});

test("when nothing listens and epik-app isn't installed, the error says how to install it", async () => {
  await assert.rejects(request({ type: 'ping' }), {
    message: /Epik app isn't installed[^]*cargo install --git https:\/\/github\.com\/epik-agent\/Epik epik-app/,
  });
});

test("an app that doesn't answer pings in time is an error", async () => {
  await installEpikApp('exit 0');
  await assert.rejects(request({ type: 'ping' }, { startTimeoutMs: 600 }), {
    message: /didn't start within 0\.6 seconds/,
  });
});

test("with start: false, when nothing listens, the app isn't started and the error says it isn't running", async () => {
  await installEpikApp(`exec '${process.execPath}' '${fakeEpikApp}' "$@"`);
  process.env.FAKE_APP_ARGS = join(dir, 'args.json');

  await assert.rejects(request({ type: 'list' }, { start: false }), { message: /Epik app isn't running/ });
  await assert.rejects(readFile(process.env.FAKE_APP_ARGS, 'utf8'), { code: 'ENOENT' });
});
