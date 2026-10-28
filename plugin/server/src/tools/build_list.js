import * as z from 'zod';

import { request } from '../app.js';

export default {
  name: 'build_list',
  description: `List the Epik app's latest builds, newest first.

The app isn't started: when it isn't running, this says so.

Args:
    limit: Maximum number of runs. Default 20.

Returns each run's state, as build_status gives it.`,
  inputSchema: {
    limit: z.number().int().positive().optional(),
  },
  run: async ({ limit }) => (await request({ type: 'list', limit }, { start: false })).runs,
};
