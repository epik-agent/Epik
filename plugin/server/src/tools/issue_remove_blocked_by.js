import { edgeInput, edges, issueId } from '../blocked_by.js';
import { gh } from '../gh.js';

export default {
  name: 'issue_remove_blocked_by',
  description: `Remove a blocked-by relationship between two issues.

${edgeInput.args}`,
  inputSchema: edgeInput.inputSchema,
  run: async ({ repo, issue_number, blocked_by_number, blocked_by_repo = repo }) => {
    const id = await issueId(blocked_by_repo, blocked_by_number);
    await gh(['api', '--method', 'DELETE', `${edges(repo, issue_number)}/${id}`]);
    return { repo, issue_number, removed_blocked_by: { repo: blocked_by_repo, number: blocked_by_number } };
  },
};
