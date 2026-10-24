import * as z from 'zod';

import { GhError, graphql } from '../gh.js';
import { issueInProject, issueItem, statusField } from '../projects.js';

export default {
  name: 'project_set_status',
  description: `Set an issue's Status in a GitHub Project, adding the issue to the project if it is not in it.

Args:
    owner: The user or organization login that owns the project.
    number: The project number, from its URL or project_list.
    repo: The issue's repository, as owner/name.
    issue_number: The issue number.
    status_name: The Status option, e.g. "In Progress" or "Done".

Returns { item_id, status }.`,
  inputSchema: { ...issueInProject, status_name: z.string() },
  run: async ({ status_name, ...input }) => {
    const { owner, number } = input;
    const [{ projectId, field }, { issueId, item }] = await Promise.all([statusField(owner, number), issueItem(input)]);
    const option = field.options.find(({ name }) => name === status_name);
    if (!option) {
      const statuses = field.options.map(({ name }) => name).join(', ');
      throw new GhError(`Project ${owner} #${number} has no status "${status_name}". Its statuses are ${statuses}.`);
    }
    const itemId = item?.id ?? (await add(projectId, issueId));
    await graphql(
      `mutation($project: ID!, $item: ID!, $field: ID!, $option: String!) {
        updateProjectV2ItemFieldValue(input: {
          projectId: $project, itemId: $item, fieldId: $field, value: { singleSelectOptionId: $option }
        }) { projectV2Item { id } }
      }`,
      { project: projectId, item: itemId, field: field.id, option: option.id },
    );
    return { item_id: itemId, status: status_name };
  },
};

async function add(projectId, issueId) {
  const data = await graphql(
    `mutation($project: ID!, $content: ID!) {
      addProjectV2ItemById(input: { projectId: $project, contentId: $content }) { item { id } }
    }`,
    { project: projectId, content: issueId },
  );
  return data.addProjectV2ItemById.item.id;
}
