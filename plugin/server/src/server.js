import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';

// An MCP server named `epik` offering `tools`. Each tool is
// `{ name, description, inputSchema, run }`, where `inputSchema` is a shape of
// zod schemas and `run` resolves to a string, which the caller gets as is, or
// to any other JSON value, which the caller gets as JSON text. When `run`
// throws, the SDK gives the caller an MCP error carrying the thrown message.
export function createServer(tools) {
  const server = new McpServer({ name: 'epik', version: '1.0.0' });
  for (const { name, description, inputSchema, run } of tools) {
    server.registerTool(name, { description, inputSchema }, async (input) => {
      const result = await run(input);
      return { content: [{ type: 'text', text: typeof result === 'string' ? result : JSON.stringify(result) }] };
    });
  }
  return server;
}
