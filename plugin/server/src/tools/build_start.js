import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, open } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';

import * as z from 'zod';

import { gh } from '../gh.js';
import { get, list } from '../rest.js';

const CLONE_TIMEOUT_SECONDS = 10 * 60;

// The settings that turn off an installed copy of the plugin, so a build
// loads only the copy named by --plugin-dir.
const ONLY_PLUGIN_DIR = JSON.stringify({ enabledPlugins: { 'epik@epik': false } });

// The branch name part made from `title`: lowercase, each run of characters
// other than a-z and 0-9 replaced by '-', cut to 40 characters, with '-'
// trimmed from both ends.
export function slug(title) {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .slice(0, 40)
    .replace(/^-+|-+$/g, '');
}

// The default branch for epic `number` titled `title`: epic/<number>-<slug>,
// or epic/<number> when the title has no letters or digits to slug.
export function epicBranch(number, title) {
  const tail = slug(title);
  return tail ? `epic/${number}-${tail}` : `epic/${number}`;
}

// `date` as YYYYMMDDTHHMMSSZ, in UTC.
export function stamp(date) {
  return date.toISOString().replace(/[-:]/g, '').replace(/\.\d+/, '');
}

// The arguments to `claude` that run `command` headless with the plugin at
// `pluginRoot`.
export function claudeArgs(command, pluginRoot) {
  return [
    '-p', command,
    '--output-format', 'stream-json', '--verbose', '--forward-subagent-text',
    '--permission-mode', 'auto', '--permission-prompts', 'none',
    '--plugin-dir', pluginRoot,
    '--settings', ONLY_PLUGIN_DIR,
  ];
}

// Starts `claude` with `args` in `cwd`, detached, writing stdout to `log` and
// stderr to `errorsLog`. Resolves to its pid once it has started.
async function launch(args, cwd, log, errorsLog) {
  const out = await open(log, 'a');
  const err = await open(errorsLog, 'a');
  try {
    const child = spawn('claude', args, { cwd, detached: true, stdio: ['ignore', out.fd, err.fd] });
    // once() rejects when the child emits 'error' before 'spawn'.
    const error = await once(child, 'spawn').then(
      () => null,
      (e) => e,
    );
    if (error) {
      throw error.code === 'ENOENT'
        ? new Error('Claude Code is not installed. Install it from https://claude.com/claude-code and sign in.')
        : error;
    }
    // The build outlives this call; its later failures show in its logs.
    child.on('error', () => {});
    child.unref();
    return child.pid;
  } finally {
    await out.close();
    await err.close();
  }
}

export default {
  name: 'build_start',
  description: `Start a local Claude Code build of a GitHub issue or epic, and return at once.

This is how to implement an issue or an epic. Never implement one in this
conversation: when the user asks to build, implement or work on an issue or
an epic, call this tool.

An issue with sub-issues is an epic and is built with /epik:epic; any other
issue is built with /epik:issue. The build runs headless in a fresh clone
under ~/.epik/builds/ and streams its output to the log this returns; the
user can follow it with \`tail -f\`. The build opens pull requests, merges
them and closes the issues itself.

Args:
    repo: Repository in owner/name format.
    number: The issue or epic issue to build.
    branch: For an epic, the epic branch, created if missing; default
        epic/<number>-<slug of its title>. For an issue, the branch its pull
        request merges into; default the repository's default branch.

Returns run_id, kind (epic or issue), command (the slash command the build
runs), branch, clone, log, errors_log and pid.`,
  inputSchema: {
    repo: z.string(),
    number: z.number().int().positive(),
    branch: z.string().min(1).optional(),
  },
  run: async ({ repo, number, branch }) => {
    const pluginRoot = process.env.CLAUDE_PLUGIN_ROOT;
    if (!pluginRoot) throw new Error('CLAUDE_PLUGIN_ROOT is not set, so the build cannot load the Epik plugin.');

    const issue = await get(`repos/${repo}/issues/${number}`);
    if (issue.pull_request) throw new Error(`#${number} in ${repo} is a pull request, not an issue.`);
    if (issue.state !== 'open') throw new Error(`#${number} in ${repo} is ${issue.state}.`);

    const kind = (await list(`repos/${repo}/issues/${number}/sub_issues`)).length > 0 ? 'epic' : 'issue';
    const target = branch ?? (kind === 'epic' ? epicBranch(number, issue.title) : (await get(`repos/${repo}`)).default_branch);
    const command = `/epik:${kind} ${number} ${target}`;

    const home = join(homedir(), '.epik');
    const runId = `${repo.replace('/', '-')}-${number}-${stamp(new Date())}`;
    const clone = join(home, 'builds', runId);
    const runs = join(home, 'runs');
    await mkdir(clone, { recursive: true });
    await mkdir(runs, { recursive: true });
    await gh(['repo', 'clone', repo, clone], { timeoutSeconds: CLONE_TIMEOUT_SECONDS });

    const log = join(runs, `${runId}.jsonl`);
    const errorsLog = join(runs, `${runId}.err`);
    const pid = await launch(claudeArgs(command, pluginRoot), clone, log, errorsLog);
    return { run_id: runId, kind, command, branch: target, clone, log, errors_log: errorsLog, pid };
  },
};
