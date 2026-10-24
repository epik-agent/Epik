// Puts a fake `gh` on PATH so tests control what GitHub "says".

import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Replaces PATH with a directory holding a `gh` shell script whose body is
// `script`. Returns a function that restores the original PATH and removes
// the directory.
export async function fakeGh(script) {
  const dir = await mkdtemp(join(tmpdir(), 'fake-gh-'));
  const gh = join(dir, 'gh');
  await writeFile(gh, `#!/bin/sh\n${script}\n`);
  await chmod(gh, 0o755);
  return onPath(dir);
}

// Replaces PATH with an empty directory, so `gh` is not installed.
export async function noGh() {
  return onPath(await mkdtemp(join(tmpdir(), 'no-gh-')));
}

function onPath(dir) {
  const original = process.env.PATH;
  process.env.PATH = dir;
  return async () => {
    process.env.PATH = original;
    await rm(dir, { recursive: true, force: true });
  };
}
