// Every GitHub operation goes through the gh CLI, and through this module.

import { execFile } from 'node:child_process';

const TIMEOUT_SECONDS = 60;
const MAX_BUFFER_MB = 64;

// The most gh processes that run at once. A tool that fans out across an
// epic's issues stays under GitHub's secondary rate limits.
export const MAX_PROCESSES = 8;

// A gh call that failed. The message is what the caller sees.
export class GhError extends Error {
  name = 'GhError';
}

// Runs gh with `args`, returning stdout, or its parsed value when `json` is
// set. Throws a GhError carrying gh's stderr and exit code when gh fails, or
// when it runs longer than TIMEOUT_SECONDS.
export async function gh(args, { json = false } = {}) {
  const { exit_code, stdout, stderr } = await run(args);
  if (exit_code !== 0) throw new GhError(explain(exit_code, stderr));
  return json ? JSON.parse(stdout) : stdout;
}

// Runs a GraphQL `query` and resolves to its data. gh sends each `-f`/`-F`
// field as its own variable, so each variable gets one: `-F` for integers,
// which gh sends as numbers, and `-f` for strings. Undefined ones are left out.
export async function graphql(query, variables = {}) {
  const fields = Object.entries(variables)
    .filter(([, value]) => value !== undefined)
    .flatMap(([name, value]) => [Number.isInteger(value) ? '-F' : '-f', `${name}=${value}`]);
  const { data } = await gh(['api', 'graphql', '-f', `query=${query}`, ...fields], { json: true });
  return data;
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

let running = 0;
const waiting = [];

// Resolves to `start()`'s result once one of the MAX_PROCESSES slots is free.
// A finishing call hands its slot straight to the next waiting one.
async function throttled(start) {
  if (running < MAX_PROCESSES) running++;
  else await new Promise((resolve) => waiting.push(resolve));
  try {
    return await start();
  } finally {
    const next = waiting.shift();
    if (next) next();
    else running--;
  }
}

// Resolves to gh's exit code and output. Throws a GhError when gh never
// produced an exit code: it is not installed, printed too much, ran out of
// time, or was stopped by a signal.
function run(args) {
  const command = `gh ${args.join(' ')}`;
  const options = { timeout: TIMEOUT_SECONDS * 1000, maxBuffer: MAX_BUFFER_MB * 1024 * 1024 };
  return throttled(
    () =>
      new Promise((resolve, reject) => {
        execFile('gh', args, options, (error, stdout, stderr) => {
          if (error?.code === 'ENOENT') {
            reject(new GhError('gh is not installed. Install it from https://cli.github.com/'));
          } else if (error?.code === 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER') {
            reject(new GhError(`gh printed more than ${MAX_BUFFER_MB} MB: ${command}`));
          } else if (error?.killed) {
            reject(new GhError(`gh timed out after ${TIMEOUT_SECONDS} seconds: ${command}`));
          } else if (error && typeof error.code !== 'number') {
            reject(new GhError(`gh was stopped by ${error.signal ?? error.message}: ${command}`));
          } else {
            resolve({ exit_code: error?.code ?? 0, stdout, stderr });
          }
        });
      }),
  );
}

function explain(exitCode, stderr) {
  const signedOut = stderr.includes('gh auth login') ? 'gh is not signed in. Run `gh auth login`.\n' : '';
  return `${signedOut}${stderr.trim()}\n(exit code ${exitCode})`;
}
