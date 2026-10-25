// A fake `gh` for tools that make several calls: it records each call's
// arguments and answers with the first reply whose needle the call contains.

import { readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { fakeGh } from './fake-gh.js';

const responder = fileURLToPath(new URL('./fake-github-gh.js', import.meta.url));

// `replies` is a list of `[needle, body]`. A call whose arguments, joined by
// spaces, contain `needle` prints `body` as JSON. Returns `{ calls, restore }`:
// `calls()` resolves to each call's argument list, in order.
export async function fakeGithub(replies) {
  const log = join(tmpdir(), `fake-github-${process.pid}-${Date.now()}.jsonl`);
  process.env.FAKE_GITHUB_REPLIES = JSON.stringify(replies);
  process.env.FAKE_GITHUB_LOG = log;
  const restorePath = await fakeGh(`exec '${process.execPath}' '${responder}' "$@"`);
  return {
    calls: async () =>
      (await readFile(log, 'utf8'))
        .trim()
        .split('\n')
        .map((line) => JSON.parse(line)),
    restore: async () => {
      await restorePath();
      await rm(log, { force: true });
    },
  };
}

// The `gh api` arguments that fetch every page of `path`.
export const pages = (path) => `api --paginate --slurp ${path}${path.includes('?') ? '&' : '?'}per_page=100`;
