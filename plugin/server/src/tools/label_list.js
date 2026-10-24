import * as z from 'zod';

import { gh } from '../gh.js';

export default {
  name: 'label_list',
  description: `List every label in a repository with its color and description.

Args:
    repo: Repository in owner/name format.`,
  inputSchema: { repo: z.string() },
  run: ({ repo }) => gh(['label', 'list', '--repo', repo, '--json', 'name,color,description'], { json: true }),
};
