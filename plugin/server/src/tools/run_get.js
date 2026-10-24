import * as z from 'zod';

import { gh } from '../gh.js';
import { RUN_FIELDS } from './run_list.js';

const FIELDS = [...RUN_FIELDS, 'event', 'headSha', 'createdAt', 'updatedAt'];

export default {
  name: 'run_get',
  description: `Get one GitHub Actions run, with its event, head SHA and timestamps.

Args:
    repo: Repository in owner/name format.
    run_id: The run's database ID.`,
  inputSchema: { repo: z.string(), run_id: z.number().int() },
  run: ({ repo, run_id }) =>
    gh(['run', 'view', String(run_id), '--repo', repo, '--json', FIELDS.join(',')], { json: true }),
};
