import * as z from 'zod';

import { gh } from '../gh.js';

export default {
  name: 'label_delete',
  description: `Delete a label from a repository.

Args:
    repo: Repository in owner/name format.
    name: Label name.`,
  inputSchema: { repo: z.string(), name: z.string().min(1) },
  run: async ({ repo, name }) => {
    await gh(['label', 'delete', name, '--repo', repo, '--yes']);
    return { deleted: name, repo };
  },
};
