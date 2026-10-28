import * as z from 'zod';

import { request } from '../app.js';

export default {
  name: 'build_status',
  description: `Ask the Epik app where a build is: the run's state and each of its issues'.

The app isn't started: when it isn't running, this says so.

Args:
    run_id: The run, as build_start or build_list gives it.

Returns the run's repo, number, title, kind (epic or issue), branch, status
(running, succeeded, failed or interrupted), reason (why it failed or was
interrupted), pull_request, started_at, ended_at, elapsed_ms and cost_usd, and
its members: each issue's number, title, blocked_by, status (waiting, running,
succeeded, merged or failed), reason, pull_request, duration_ms and cost_usd.`,
  inputSchema: {
    run_id: z.string().min(1),
  },
  run: async ({ run_id }) => {
    const { type, ...run } = await request({ type: 'status', run_id }, { start: false });
    return run;
  },
};
