// Every GitHub operation goes through the gh CLI, and through this module.

import { execFile } from 'node:child_process';

const TIMEOUT_SECONDS = 60;

// A gh call that failed. The message is what the caller sees.
export class GhError extends Error {
  name = 'GhError';
}

// Runs gh with `args`, returning stdout, or its parsed value when `json` is
// set. Throws a GhError carrying gh's stderr and exit code when gh fails.
export async function gh(args, { json = false } = {}) {
  const { exit_code, stdout, stderr } = await run(args);
  if (exit_code !== 0) throw new GhError(explain(exit_code, stderr));
  return json ? JSON.parse(stdout) : stdout;
}

// Runs gh with `args` and reports the outcome, failure included, as a value.
export async function ghRaw(args) {
  try {
    const result = await run(args);
    return { ok: result.exit_code === 0, ...result };
  } catch (error) {
    if (!(error instanceof GhError)) throw error;
    return { ok: false, exit_code: -1, stdout: '', stderr: error.message };
  }
}

// Resolves to gh's exit code and output. Throws a GhError when gh never
// produced an exit code: it is not installed, or it ran out of time.
function run(args) {
  const options = { timeout: TIMEOUT_SECONDS * 1000, maxBuffer: 64 * 1024 * 1024 };
  return new Promise((resolve, reject) => {
    execFile('gh', args, options, (error, stdout, stderr) => {
      if (error?.code === 'ENOENT') {
        reject(new GhError('gh is not installed. Install it from https://cli.github.com/'));
      } else if (error?.killed) {
        reject(new GhError(`gh timed out after ${TIMEOUT_SECONDS} seconds: gh ${args.join(' ')}`));
      } else if (error && typeof error.code !== 'number') {
        reject(error);
      } else {
        resolve({ exit_code: error?.code ?? 0, stdout, stderr });
      }
    });
  });
}

function explain(exitCode, stderr) {
  const signedOut = stderr.includes('gh auth login') ? 'gh is not signed in. Run `gh auth login`.\n' : '';
  return `${signedOut}${stderr.trim()}\n(exit code ${exitCode})`;
}
