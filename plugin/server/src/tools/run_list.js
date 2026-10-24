import * as z from 'zod';

import { gh } from '../gh.js';

export const RUN_FIELDS = ['databaseId', 'name', 'workflowName', 'headBranch', 'status', 'conclusion', 'url'];

export default {
  name: 'run_list',
  description: `List a repository's GitHub Actions runs, newest first.

Args:
    repo: Repository in owner/name format.
    workflow: Only runs of this workflow, by name or filename.
    branch: Only runs on this branch.
    status: Only runs with this status or conclusion (e.g. in_progress, failure).
    limit: Maximum number of runs. Default 20.`,
  inputSchema: {
    repo: z.string(),
    workflow: z.string().optional(),
    branch: z.string().optional(),
    status: z.string().optional(),
    limit: z.number().int().positive().optional(),
  },
  run: ({ repo, workflow, branch, status, limit = 20 }) =>
    gh(
      [
        'run', 'list', '--repo', repo, '--limit', String(limit),
        ...(workflow ? ['--workflow', workflow] : []),
        ...(branch ? ['--branch', branch] : []),
        ...(status ? ['--status', status] : []),
        '--json', RUN_FIELDS.join(','),
      ],
      { json: true },
    ),
};
