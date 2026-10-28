// A fake Epik app: a socket server that answers each request line with
// `respond(request)`, and records the requests it gets.

import { createServer } from 'node:net';
import { once } from 'node:events';

// Listens on `path`. Returns `{ requests, close }`.
export async function fakeApp(path, respond) {
  const requests = [];
  const server = createServer((socket) => {
    let buffer = '';
    socket.setEncoding('utf8');
    socket.on('data', (chunk) => {
      buffer += chunk;
      const end = buffer.indexOf('\n');
      if (end < 0) return;
      const request = JSON.parse(buffer.slice(0, end));
      requests.push(request);
      socket.end(`${JSON.stringify(respond(request))}\n`);
    });
  });
  server.listen(path);
  await once(server, 'listening');
  return { requests, close: () => new Promise((resolve) => server.close(resolve)) };
}

// What the app replies to `build`.
export function started(request) {
  const run_id = `${request.repo.replace('/', '-')}-${request.number}-20261028T212859Z`;
  return {
    type: 'started',
    run_id,
    kind: 'issue',
    branch: request.branch ?? 'main',
    run_dir: `/home/me/.epik/runs/${run_id}`,
  };
}
