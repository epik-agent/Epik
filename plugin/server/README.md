# Epik server

The MCP server inside the plugin. It provides the GitHub operations the
standard GitHub connector lacks, running each one through the `gh` CLI.

- `src/gh.js` runs `gh`. Every tool goes through it.
- `src/tools/` holds one module per tool. `src/tools/index.js` lists them.
- `npm test` runs the tests with Node's test runner.
- `npm run build` bundles the server into `dist/index.js`. That file is
  committed, so users need Node and nothing else. Rebuild it with every
  source change.
