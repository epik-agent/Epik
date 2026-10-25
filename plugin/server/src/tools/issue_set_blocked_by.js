import { edgeInput, edges, issueId } from '../blocked_by.js';
import { gh } from '../gh.js';

export default {
  name: 'issue_set_blocked_by',
  description: `Mark an issue as blocked by another issue.

${edgeInput.args}`,
  inputSchema: edgeInput.inputSchema,
  run: async ({ repo, issue_number, blocked_by_number, blocked_by_repo = repo }) => {
    const id = await issueId(blocked_by_repo, blocked_by_number);
    await gh(['api', '--method', 'POST', edges(repo, issue_number), '-F', `issue_id=${id}`]);
    return { repo, issue_number, blocked_by: { repo: blocked_by_repo, number: blocked_by_number } };
  },
};
