import { join } from 'node:path';

import * as z from 'zod';

import { request } from '../app.js';

export default {
  name: 'build_start',
  description: `Ask the Epik app to build a GitHub issue or epic, and return once the build has started.

This is how to implement an issue or an epic. Never implement one in this
conversation: when the user asks to build, implement or work on an issue or
an epic, call this tool.

The Epik app runs the build, and is started first if it isn't running. An
issue with sub-issues is built as an epic: the app runs its issues in
dependency order, one coding agent each, and merges each issue's pull
request into the epic branch itself; the epic's pull request into the
repository's default branch waits for the user to review and merge it. Any
other issue is built alone, and its pull request into the default branch
waits for the user. The user can watch the build from the Epik app's tray
icon, or with \`tail -f\` on the events log this returns.

Args:
    repo: Repository in owner/name format.
    number: The issue or epic issue to build.
    branch: The branch the pull requests merge into. Default
        epic/<number>-<slug of its title> for an epic, and the repository's
        default branch for an issue.

Returns run_id, kind (epic or issue), branch, run_dir (the run's record) and
events (the run's events log, in run_dir).`,
  inputSchema: {
    repo: z.string(),
    number: z.number().int().positive(),
    branch: z.string().min(1).optional(),
  },
  run: async ({ repo, number, branch }) => {
    const pluginDir = process.env.CLAUDE_PLUGIN_ROOT;
    if (!pluginDir) throw new Error('CLAUDE_PLUGIN_ROOT is not set, so the build cannot load the Epik plugin.');

    const reply = await request({ type: 'build', repo, number, branch, plugin_dir: pluginDir });
    return {
      run_id: reply.run_id,
      kind: reply.kind,
      branch: reply.branch,
      run_dir: reply.run_dir,
      events: join(reply.run_dir, 'events.jsonl'),
    };
  },
};
