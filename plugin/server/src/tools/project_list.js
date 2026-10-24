import * as z from 'zod';

import { paginate, projectOwner } from '../projects.js';

export default {
  name: 'project_list',
  description: `List a GitHub user's or organization's open Projects.

Use this to find a project's number for the other project tools.

Args:
    owner: The user or organization login that owns the projects.

Returns a list of { number, title, url }.`,
  inputSchema: { owner: z.string() },
  run: async ({ owner }) => {
    const projects = await paginate(
      (cursor) =>
        projectOwner(
          owner,
          `projectsV2(first: 100, after: $cursor) {
            pageInfo { hasNextPage endCursor }
            nodes { number title url closed }
          }`,
          { cursor },
        ),
      (data) => data.projectsV2,
    );
    return projects.filter(({ closed }) => !closed).map(({ number, title, url }) => ({ number, title, url }));
  },
};
