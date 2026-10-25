import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';

import projectGetItem from '../src/tools/project_get_item.js';
import projectList from '../src/tools/project_list.js';
import projectListItems from '../src/tools/project_list_items.js';
import projectSetStatus from '../src/tools/project_set_status.js';
import { fakeGithub } from './fake-github.js';

let github;
afterEach(() => github.restore());

// The call whose arguments, joined by spaces, contain `needle`.
const call = async (needle) => (await github.calls()).find((args) => args.join(' ').includes(needle));

const owned = (selection) => ({ data: { repositoryOwner: selection } });
const page = (nodes, endCursor = null) => ({ pageInfo: { hasNextPage: endCursor !== null, endCursor }, nodes });
const noProjects = ['projectsV2', owned({ projectsV2: page([]) })];

const item = (number, status) => ({
  content: { number, title: `Issue ${number}`, repository: { nameWithOwner: 'epik-agent/Epik' } },
  fieldValues: { nodes: [{ value: status, field: { name: 'Status' } }, {}] },
});

const statusField = [
  'field(name: "Status")',
  owned({
    projectV2: {
      id: 'project-12',
      field: { id: 'field-status', options: [
        { id: 'opt-todo', name: 'Todo' },
        { id: 'opt-progress', name: 'In Progress' },
        { id: 'opt-done', name: 'Done' },
      ] },
    },
  }),
];

const issue311 = { owner: 'epik-agent', number: 12, repo: 'epik-agent/Epik', issue_number: 311 };
const project = (number, login = 'epik-agent') => ({ number, owner: { login } });
const issueItems = (nodes) => ['projectItems', { data: { repository: { issue: { id: 'issue-311', projectItems: { nodes } } } } }];
const inProject12 = issueItems([{ id: 'item-311', project: project(12, 'Epik-Agent'), fieldValues: { nodes: [] } }]);

test('user and organization owners are both queried as a ProjectV2Owner', async () => {
  github = await fakeGithub([noProjects]);
  await projectList.run({ owner: 'epik-agent' });
  await projectList.run({ owner: 'wpm' });
  const [organization, user] = (await github.calls()).map((args) => args.slice(3));
  assert.match(organization[0], /repositoryOwner\(login: \$owner\) \{ \.\.\. on ProjectV2Owner/);
  assert.deepEqual(organization.slice(1), ['-f', 'owner=epik-agent']);
  assert.deepEqual(user, [organization[0], '-f', 'owner=wpm']);
});

test('an owner that does not exist is named in the error', async () => {
  github = await fakeGithub([['projectsV2', owned(null)]]);
  await assert.rejects(projectList.run({ owner: 'nobody' }), {
    name: 'GhError',
    message: 'There is no GitHub user or organization named nobody.',
  });
});

test('project_list returns the open projects across every page', async () => {
  const listed = (number, closed) => ({ number, title: `P${number}`, url: `https://github.com/orgs/epik-agent/projects/${number}`, closed });
  github = await fakeGithub([
    ['cursor=page-2', owned({ projectsV2: page([listed(12, false)]) })],
    ['projectsV2', owned({ projectsV2: page([listed(3, true), listed(7, false)], 'page-2') })],
  ]);
  assert.deepEqual(await projectList.run({ owner: 'epik-agent' }), [
    { number: 7, title: 'P7', url: 'https://github.com/orgs/epik-agent/projects/7' },
    { number: 12, title: 'P12', url: 'https://github.com/orgs/epik-agent/projects/12' },
  ]);
});

test('project_list_items returns each issue with its status, filtered by status', async () => {
  github = await fakeGithub([
    ['items(', owned({ projectV2: { items: page([item(1, 'Done'), { content: {}, fieldValues: { nodes: [] } }, item(2, 'Todo')]) } })],
  ]);
  assert.deepEqual(await projectListItems.run({ owner: 'epik-agent', number: 12, status_filter: 'Todo' }), [
    { issue_number: 2, title: 'Issue 2', url: 'https://github.com/epik-agent/epik/issues/2', repo: 'epik-agent/Epik', status: 'Todo' },
  ]);
  assert.deepEqual((await call('items(')).slice(-4), ['-f', 'owner=epik-agent', '-F', 'number=12']);
});

test("project_get_item returns the item's field values", async () => {
  github = await fakeGithub([
    issueItems([
      { id: 'other-owner', project: project(12, 'wpm'), fieldValues: { nodes: [] } },
      { id: 'item-311', project: project(12), fieldValues: { nodes: [
        { value: 'In Progress', field: { name: 'Status' } },
        { value: 'The Node server reads and sets GitHub Project status', field: { name: 'Title' } },
        { value: 3, field: { name: 'Estimate' } },
        {},
      ] } },
    ]),
  ]);
  assert.deepEqual(await projectGetItem.run(issue311), {
    found: true,
    item_id: 'item-311',
    fields: { Status: 'In Progress', Title: 'The Node server reads and sets GitHub Project status', Estimate: 3 },
  });
  assert.deepEqual((await call('projectItems')).slice(-6), ['-f', 'owner=epik-agent', '-f', 'name=Epik', '-F', 'number=311']);
});

test('project_get_item reports an issue outside the project as not found', async () => {
  github = await fakeGithub([issueItems([{ id: 'item-7', project: project(7), fieldValues: { nodes: [] } }])]);
  assert.deepEqual(await projectGetItem.run(issue311), { found: false });
});

const updated = ['updateProjectV2ItemFieldValue', { data: { updateProjectV2ItemFieldValue: { projectV2Item: { id: 'item-311' } } } }];

test('project_set_status sets the Status option on the existing item', async () => {
  github = await fakeGithub([updated, statusField, inProject12]);
  assert.deepEqual(await projectSetStatus.run({ ...issue311, status_name: 'Done' }), { item_id: 'item-311', status: 'Done' });
  assert.equal(await call('addProjectV2ItemById'), undefined);
  assert.deepEqual((await call('updateProjectV2ItemFieldValue')).slice(-8), [
    '-f', 'project=project-12', '-f', 'item=item-311', '-f', 'field=field-status', '-f', 'option=opt-done',
  ]);
});

test('project_set_status adds an issue that is not yet in the project', async () => {
  github = await fakeGithub([
    updated,
    ['addProjectV2ItemById', { data: { addProjectV2ItemById: { item: { id: 'item-new' } } } }],
    statusField,
    issueItems([]),
  ]);
  assert.deepEqual(await projectSetStatus.run({ ...issue311, status_name: 'Todo' }), { item_id: 'item-new', status: 'Todo' });
  assert.deepEqual((await call('addProjectV2ItemById')).slice(-4), ['-f', 'project=project-12', '-f', 'content=issue-311']);
  assert.deepEqual((await call('updateProjectV2ItemFieldValue')).slice(-6, -4), ['-f', 'item=item-new']);
});

test('an unknown status names the available ones', async () => {
  github = await fakeGithub([statusField, inProject12]);
  await assert.rejects(projectSetStatus.run({ ...issue311, status_name: 'Blocked' }), {
    name: 'GhError',
    message: 'Project epik-agent #12 has no status "Blocked". Its statuses are Todo, In Progress, Done.',
  });
});

test('a project without a Status field says so', async () => {
  github = await fakeGithub([['field(name: "Status")', owned({ projectV2: { id: 'project-12', field: null } })], inProject12]);
  await assert.rejects(projectSetStatus.run({ ...issue311, status_name: 'Done' }), {
    message: 'Project epik-agent #12 has no Status field.',
  });
});

test('a project that does not exist is named in the error', async () => {
  github = await fakeGithub([['projectV2(number', owned({ projectV2: null })], inProject12]);
  const missing = { name: 'GhError', message: 'epik-agent has no project #999, or it is not visible to you.' };
  await assert.rejects(projectListItems.run({ owner: 'epik-agent', number: 999 }), missing);
  await assert.rejects(projectSetStatus.run({ ...issue311, number: 999, status: 'Done' }), missing);
});
