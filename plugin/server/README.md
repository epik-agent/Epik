# Epik server

The MCP server inside the plugin. It runs Epik's GitHub operations through
the `gh` CLI.

- `src/gh.js` runs `gh`. Every GitHub operation goes through it.
- `src/app.js` talks to the Epik app, which runs builds. `build_start`,
  `build_status` and `build_list` hand builds to the app and ask it about
  them, over the local socket in
  [`docs/app-protocol.md`](../../docs/app-protocol.md). The app is started
  if it isn't running.
- `src/tools/` holds one module per tool. `src/tools/index.js` lists them.
- `npm test` runs the tests with Node's test runner.
- `npm run build` bundles the server into `dist/index.js`. That file is
  committed, so users need Node and nothing else. Rebuild it with every
  source change.
