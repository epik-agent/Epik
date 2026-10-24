import { fieldValues, issueInProject, issueItem } from '../projects.js';

export default {
  name: 'project_get_item',
  description: `Get an issue's item in a GitHub Project, with its field values.

Args:
    owner: The user or organization login that owns the project.
    number: The project number, from its URL or project_list.
    repo: The issue's repository, as owner/name.
    issue_number: The issue number.

Returns { found: true, item_id, fields: { field name: value } }, or
{ found: false } when the issue is not in the project.`,
  inputSchema: issueInProject,
  run: async (input) => {
    const { item } = await issueItem(input);
    return item ? { found: true, item_id: item.id, fields: fieldValues(item) } : { found: false };
  },
};
