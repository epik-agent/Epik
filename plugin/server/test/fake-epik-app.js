// A fake `epik-app` executable: it records its arguments in FAKE_APP_ARGS,
// waits a little, as the app does while it starts, then serves the socket at
// EPIK_SOCKET until it has answered a build request.

import { writeFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';

import { fakeApp, started } from './fake-app.js';

writeFileSync(process.env.FAKE_APP_ARGS, JSON.stringify(process.argv.slice(2)));
await sleep(400);
const app = await fakeApp(process.env.EPIK_SOCKET, (request) => {
  if (request.type === 'ping') return { type: 'pong', version: '0.1.0' };
  setImmediate(() => app.close());
  return started(request);
});
