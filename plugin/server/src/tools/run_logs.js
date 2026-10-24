import * as z from 'zod';

import { gh } from '../gh.js';

export default {
  name: 'run_logs',
  description: `Get the logs of a GitHub Actions run, of one of its jobs, or of only its failed steps.

Args:
    repo: Repository in owner/name format.
    run_id: The run's database ID.
    job_id: Only this job's logs.
    failed_only: Only the failed steps' logs. Default false.`,
  inputSchema: {
    repo: z.string(),
    run_id: z.number().int(),
    job_id: z.number().int().optional(),
    failed_only: z.boolean().optional(),
  },
  run: ({ repo, run_id, job_id, failed_only }) =>
    gh([
      'run', 'view', String(run_id), '--repo', repo,
      ...(job_id ? ['--job', String(job_id)] : []),
      failed_only ? '--log-failed' : '--log',
    ]),
};
