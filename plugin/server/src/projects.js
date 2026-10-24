// GitHub Projects, which only GraphQL reaches. Every call looks up the IDs it
// needs afresh: one extra query is cheaper than a cache that goes stale.
// Users and organizations are both ProjectV2Owners, so one query serves both.

import * as z from 'zod';

import { GhError, graphql } from './gh.js';

// The input naming an issue and a project it may be in.
export const issueInProject = {
  owner: z.string(),
  number: z.number().int(),
  repo: z.string().regex(/^[^/]+\/[^/]+$/, 'repo is owner/name'),
  issue_number: z.number().int(),
};

const VARIABLE_TYPES = { number: 'Int!', cursor: 'String' };

// Selects `selection` on the ProjectV2Owner `owner` and resolves to it.
// `variables` may hold a `number` and a `cursor`.
export async function projectOwner(owner, selection, variables = {}) {
  const data = await graphql(
    `query($owner: String!${Object.keys(variables).map((name) => `, $${name}: ${VARIABLE_TYPES[name]}`).join('')}) {
      repositoryOwner(login: $owner) { ... on ProjectV2Owner { ${selection} } }
    }`,
    { owner, ...variables },
  );
  if (!data.repositoryOwner) throw new GhError(`There is no GitHub user or organization named ${owner}.`);
  return data.repositoryOwner;
}

// Every node of a paginated connection. `connection` picks the connection out
// of a page's data; the query takes a `$cursor`.
export async function paginate(query, connection) {
  const nodes = [];
  let cursor;
  do {
    const { pageInfo, nodes: page } = connection(await query(cursor));
    nodes.push(...page);
    cursor = pageInfo.hasNextPage ? pageInfo.endCursor : undefined;
  } while (cursor);
  return nodes;
}

const FIELD_VALUE_TYPES = { SingleSelect: 'name', Text: 'text', Number: 'number', Date: 'date', Iteration: 'title' };

// Selects an item's field values, each as `value`, for `fieldValues`.
export const FIELD_VALUES = `fieldValues(first: 50) {
  nodes {
    ${Object.entries(FIELD_VALUE_TYPES)
      .map(([type, value]) => `... on ProjectV2ItemField${type}Value { value: ${value} field { ... on ProjectV2FieldCommon { name } } }`)
      .join('\n    ')}
  }
}`;

// An item's field values as `{ field name: value }`.
export function fieldValues(item) {
  return Object.fromEntries(item.fieldValues.nodes.filter(({ field }) => field).map(({ field, value }) => [field.name, value]));
}

// The project's node ID and its Status field, with the field's options.
export async function statusField(owner, number) {
  const { projectV2 } = await projectOwner(
    owner,
    `projectV2(number: $number) {
      id
      field(name: "Status") { ... on ProjectV2SingleSelectField { id options { id name } } }
    }`,
    { number },
  );
  if (!projectV2.field?.id) throw new GhError(`Project ${owner} #${number} has no Status field.`);
  return { projectId: projectV2.id, field: projectV2.field };
}

// The issue's node ID and its item in the project `owner` #`number`, or null
// when the issue is not in that project.
export async function issueItem({ owner, number, repo, issue_number }) {
  const [repoOwner, name] = repo.split('/');
  const { issue } = (
    await graphql(
      `query($owner: String!, $name: String!, $number: Int!) {
        repository(owner: $owner, name: $name) {
          issue(number: $number) {
            id
            projectItems(first: 100) {
              nodes {
                id
                project { number owner { ... on User { login } ... on Organization { login } } }
                ${FIELD_VALUES}
              }
            }
          }
        }
      }`,
      { owner: repoOwner, name, number: issue_number },
    )
  ).repository;
  const item = issue.projectItems.nodes.find(
    ({ project }) => project.number === number && project.owner.login.toLowerCase() === owner.toLowerCase(),
  );
  return { issueId: issue.id, item: item ?? null };
}
