import * as z from 'zod';

import { gh } from '../gh.js';

export default {
  name: 'label_create',
  description: `Create a label in a repository, or with force update an existing one.

Args:
    repo: Repository in owner/name format.
    name: Label name.
    color: Hex color without the leading '#' (e.g. 'ff0000').
    description: Description of the label. Default "".
    force: Update the label if it already exists. Default false.`,
  inputSchema: {
    repo: z.string(),
    name: z.string().min(1),
    color: z.string().regex(/^[0-9a-fA-F]{6}$/),
    description: z.string().optional(),
    force: z.boolean().optional(),
  },
  run: async ({ repo, name, color, description = '', force }) => {
    await gh([
      'label', 'create', name, '--repo', repo, '--color', color,
      ...(description ? ['--description', description] : []),
      ...(force ? ['--force'] : []),
    ]);
    return { name, color, description };
  },
};
