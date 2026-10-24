// Epik's MCP server, over stdio.

import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';

import { createServer } from './server.js';
import { tools } from './tools/index.js';

await createServer(tools).connect(new StdioServerTransport());
