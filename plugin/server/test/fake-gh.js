// Puts a fake `gh` on PATH so tests control what GitHub "says", or another
// fake program so tests control what it does.

import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Replaces PATH with a directory holding a shell script `name` whose body is
// `script`. Returns a function that restores the original PATH and removes
// the directory.
export async function fakeProgram(name, script) {
  const dir = await mkdtemp(join(tmpdir(), `fake-${name}-`));
  const program = join(dir, name);
  await writeFile(program, `#!/bin/sh\n${script}\n`);
  await chmod(program, 0o755);
  return onPath(dir);
}

export const fakeGh = (script) => fakeProgram('gh', script);

// Replaces PATH with an empty directory, so nothing is installed.
export async function emptyPath() {
  return onPath(await mkdtemp(join(tmpdir(), 'empty-path-')));
}

export const noGh = emptyPath;

function onPath(dir) {
  const original = process.env.PATH;
  process.env.PATH = dir;
  return async () => {
    process.env.PATH = original;
    await rm(dir, { recursive: true, force: true });
  };
}

// Puts a fake `gh` on PATH that prints `stdout` when called with exactly
// `args`, and otherwise fails naming the arguments it got.
export async function expectGh(args, stdout = '') {
  return fakeGh(`[ "$*" = ${quote(args.join(' '))} ] || { echo "unexpected: gh $*" >&2; exit 1; }
printf '%s' ${quote(stdout)}`);
}

export function quote(text) {
  return `'${text.replaceAll("'", `'\\''`)}'`;
}
