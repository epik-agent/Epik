// A fake gh that answers `gh api` calls from a table, for tools that make
// many of them.

import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

import { fakeGh, quote } from './fake-gh.js';

// Puts a fake gh on PATH that prints `responses[args]` as JSON, where `args`
// is its arguments joined by spaces, and fails on any other call. Every call
// is recorded for `calls`. Returns the function that restores PATH.
export function fakeGitHub(responses) {
  const cases = Object.entries(responses).map(
    ([args, response]) => `'${args}') printf "%s\\n" ${quote(JSON.stringify(response))};;`,
  );
  return fakeGh(`echo "$*" >> "\${0%/*}/calls"
case "$*" in
${cases.join('\n')}
*) echo "HTTP 404: Not Found ($*)" >&2; exit 1;;
esac`);
}

// The argument lists the fake gh has been called with, in order.
export async function calls() {
  const log = await readFile(join(process.env.PATH, 'calls'), 'utf8');
  return log.trimEnd().split('\n');
}

// The `gh api` arguments that fetch every page of `path`.
export const pages = (path) => `api --paginate --slurp ${path}${path.includes('?') ? '&' : '?'}per_page=100`;
