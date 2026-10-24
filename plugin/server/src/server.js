import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';

// An MCP server named `epik` offering `tools`. Each tool is
// `{ name, description, inputSchema, run }`, where `inputSchema` is a shape of
// zod schemas and `run` resolves to a JSON value. When `run` throws, the SDK
// gives the caller an MCP error carrying the thrown message.
export function createServer(tools) {
  const server = new McpServer({ name: 'epik', version: '1.0.0' });
  for (const { name, description, inputSchema, run } of tools) {
    server.registerTool(name, { description, inputSchema }, async (input) => ({
      content: [{ type: 'text', text: JSON.stringify(await run(input)) }],
    }));
  }
  return server;
}
