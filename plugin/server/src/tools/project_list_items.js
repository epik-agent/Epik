import * as z from 'zod';

import { FIELD_VALUES, fieldValues, paginate, projectOwner } from '../projects.js';

export default {
  name: 'project_list_items',
  description: `List the issues in a GitHub Project, optionally only those with one status.

Args:
    owner: The user or organization login that owns the project.
    number: The project number, from its URL or project_list.
    status_filter: If given, only items whose Status is this.

Returns a list of { issue_number, title, url, repo, status }.`,
  inputSchema: {
    owner: z.string(),
    number: z.number().int(),
    status_filter: z.string().optional(),
  },
  run: async ({ owner, number, status_filter }) => {
    const items = await paginate(
      (cursor) =>
        projectOwner(
          owner,
          `projectV2(number: $number) {
            items(first: 100, after: $cursor) {
              pageInfo { hasNextPage endCursor }
              nodes {
                content { ... on Issue { number title url repository { nameWithOwner } } }
                ${FIELD_VALUES}
              }
            }
          }`,
          { number, cursor },
        ),
      (data) => data.projectV2.items,
    );
    return items
      .filter(({ content }) => content?.number)
      .map(({ content: { number, title, url, repository }, ...item }) => ({
        issue_number: number,
        title,
        url,
        repo: repository.nameWithOwner,
        status: fieldValues(item).Status ?? null,
      }))
      .filter(({ status }) => status_filter === undefined || status === status_filter);
  },
};
