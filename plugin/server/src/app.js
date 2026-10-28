// The Epik app's client. The app runs builds; this asks it to, over its
// local socket, as docs/app-protocol.md describes.

import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createConnection } from 'node:net';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

// How long a started app has to answer a ping.
const START_TIMEOUT_MS = 15 * 1000;

// How often to ping a starting app.
const PING_INTERVAL_MS = 250;

// The app's socket. EPIK_SOCKET overrides it, for tests; the app doesn't read it.
export function socketPath() {
  if (process.env.EPIK_SOCKET) return process.env.EPIK_SOCKET;
  return process.platform === 'win32' ? '\\\\.\\pipe\\epik' : join(homedir(), '.epik', 'epik.sock');
}

// A connection failed because nothing is listening on the socket.
class NotListening extends Error {}

// Sends `message` as one JSON line on a new connection and resolves to the
// one JSON line the app replies with. Throws NotListening when nothing is
// listening.
function send(message) {
  return new Promise((resolve, reject) => {
    const socket = createConnection(socketPath());
    let buffer = '';
    socket.setEncoding('utf8');
    socket.on('connect', () => socket.write(`${JSON.stringify(message)}\n`));
    socket.on('data', (chunk) => {
      buffer += chunk;
      const end = buffer.indexOf('\n');
      if (end < 0) return;
      socket.destroy();
      try {
        resolve(JSON.parse(buffer.slice(0, end)));
      } catch (error) {
        reject(error);
      }
    });
    socket.on('end', () => reject(new Error('The Epik app closed the connection without replying.')));
    socket.on('error', (error) =>
      reject(['ENOENT', 'ECONNREFUSED'].includes(error.code) ? new NotListening(error.message) : error),
    );
  });
}

// Starts `epik-app` detached, to outlive this server. With no arguments it
// runs in the tray, or headless where there is no tray.
async function start() {
  const child = spawn('epik-app', [], { detached: true, stdio: 'ignore' });
  // once() rejects when the child emits 'error' before 'spawn'.
  const error = await once(child, 'spawn').then(
    () => null,
    (e) => e,
  );
  if (error) {
    throw error.code === 'ENOENT'
      ? new Error(
          "The Epik app isn't installed, and it runs Epik's builds. Install it with " +
            '`cargo install --git https://github.com/epik-agent/Epik epik-app`.',
        )
      : error;
  }
  child.on('error', () => {});
  child.unref();
}

// Pings the app until it answers, for up to `timeoutMs`.
async function awaitApp(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      await send({ type: 'ping' });
      return;
    } catch (error) {
      if (!(error instanceof NotListening)) throw error;
    }
    if (Date.now() >= deadline) throw new Error(`The Epik app didn't start within ${timeoutMs / 1000} seconds.`);
    await sleep(PING_INTERVAL_MS);
  }
}

// Sends `message` to the app, starting the app first when it isn't running,
// and resolves to its reply. A reply of type "error" is thrown. With
// `start: false` an app that isn't running is an error instead.
export async function request(message, { start: startApp = true, startTimeoutMs = START_TIMEOUT_MS } = {}) {
  let reply;
  try {
    reply = await send(message);
  } catch (error) {
    if (!(error instanceof NotListening)) throw error;
    if (!startApp) throw new Error("The Epik app isn't running.");
    await start();
    await awaitApp(startTimeoutMs);
    reply = await send(message);
  }
  if (reply.type === 'error') throw new Error(reply.message);
  return reply;
}
