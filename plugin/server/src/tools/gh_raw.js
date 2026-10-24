import * as z from 'zod';

import { ghRaw } from '../gh.js';

export default {
  name: 'gh_raw',
  description: `Run an arbitrary gh command and return its result without raising.

Escape hatch for gh operations without a dedicated tool (e.g.
["release", "list", "--repo", "epik-agent/Epik"]). Pass arguments as a list
to avoid shell-quoting bugs; do not pass a single command string.

Args:
    args: The list of arguments passed to gh, excluding the leading "gh".`,
  inputSchema: { args: z.array(z.string()) },
  run: ({ args }) => ghRaw(args),
};
