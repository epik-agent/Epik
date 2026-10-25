// Blocked-by edges, through GitHub's issue-dependencies REST endpoints.

import * as z from 'zod';

import { get, list } from './rest.js';

// The REST path of the edges from issue `number` in `repo` to its blockers.
export const edges = (repo, number) => `repos/${repo}/issues/${number}/dependencies/blocked_by`;

// The ID of issue `number` in `repo`, which the edge endpoints take in place
// of its number.
export const issueId = async (repo, number) => (await get(`repos/${repo}/issues/${number}`)).id;

// The issues blocking issue `number` in `repo`.
export const blockers = (repo, number) => list(edges(repo, number));

// The input naming one edge, and its documentation.
export const edgeInput = {
  inputSchema: {
    repo: z.string(),
    issue_number: z.number().int(),
    blocked_by_number: z.number().int(),
    blocked_by_repo: z.string().optional(),
  },
  args: `Args:
    repo: Repository of the blocked issue, in owner/name format.
    issue_number: The issue that is blocked.
    blocked_by_number: The issue that blocks it.
    blocked_by_repo: Repository of the blocking issue. Defaults to repo.`,
};
