import * as z from 'zod';

import { blockers, repoOf } from '../blocked_by.js';

export default {
  name: 'issue_list_blocked_by',
  description: `List the issues blocking an issue.

Returns each blocking issue's number, title, state and repository.

Args:
    repo: Repository in owner/name format.
    issue_number: The blocked issue.`,
  inputSchema: { repo: z.string(), issue_number: z.number().int() },
  run: async ({ repo, issue_number }) =>
    (await blockers(repo, issue_number)).map((issue) => ({
      number: issue.number,
      title: issue.title,
      state: issue.state,
      repository: repoOf(issue),
    })),
};
