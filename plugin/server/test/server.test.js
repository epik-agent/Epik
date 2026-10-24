import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import * as z from 'zod';

import { createServer } from '../src/server.js';
import { tools as registered } from '../src/tools/index.js';

async function connect(tools) {
  const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
  await createServer(tools).connect(serverSide);
  const client = new Client({ name: 'test', version: '0' });
  await client.connect(clientSide);
  return client;
}

const echo = {
  name: 'echo',
  description: 'Returns its input.',
  inputSchema: { value: z.unknown() },
  run: async ({ value }) => value,
};

const failing = {
  name: 'failing',
  description: 'Fails the way gh does.',
  inputSchema: {},
  run: async () => {
    throw new Error('HTTP 404: Not Found\n(exit code 1)');
  },
};

test('a tool result reaches the caller as JSON text', async () => {
  const client = await connect([echo]);
  const result = await client.callTool({ name: 'echo', arguments: { value: { ok: true } } });
  assert.deepEqual(result.content, [{ type: 'text', text: '{"ok":true}' }]);
  assert.ok(!result.isError);
});

test("a failing tool is an MCP error whose text is the failure's message", async () => {
  const client = await connect([failing]);
  const result = await client.callTool({ name: 'failing', arguments: {} });
  assert.deepEqual(result, {
    content: [{ type: 'text', text: 'HTTP 404: Not Found\n(exit code 1)' }],
    isError: true,
  });
});

test('the built server answers initialize and lists exactly the registered tools', async () => {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [fileURLToPath(new URL('../dist/index.js', import.meta.url))],
  });
  const client = new Client({ name: 'test', version: '0' });
  await client.connect(transport);
  try {
    assert.equal(client.getServerVersion().name, 'epik');
    const listed = await client.listTools();
    assert.deepEqual(
      listed.tools.map((tool) => tool.name),
      registered.map((tool) => tool.name),
    );
  } finally {
    await client.close();
  }
});
