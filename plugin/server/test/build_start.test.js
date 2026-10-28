import assert from 'node:assert/strict';
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, test } from 'node:test';
import { setTimeout as sleep } from 'node:timers/promises';

import buildStart, { claudeArgs, epicBranch, slug, stamp } from '../src/tools/build_start.js';
import { fakeGithub, pages } from './fake-github.js';

let github;
let home;
const saved = {};

beforeEach(async () => {
  home = await mkdtemp(join(tmpdir(), 'build-start-home-'));
  for (const name of ['HOME', 'CLAUDE_PLUGIN_ROOT']) saved[name] = process.env[name];
  process.env.HOME = home;
  process.env.CLAUDE_PLUGIN_ROOT = '/plugins/epik';
});

afterEach(async () => {
  await github?.restore();
  github = undefined;
  for (const [name, value] of Object.entries(saved)) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  await rm(home, { recursive: true, force: true });
});

const issue = (state = 'open', extra = {}) => ({ number: 7, title: 'Add the Thing: part 2!', state, ...extra });

// GitHub's replies for building issue 7 without sub-issues into main.
const issueReplies = () => [
    [pages('repos/o/r/issues/7/sub_issues'), [[]]],
    ['api repos/o/r/issues/7', issue()],
    ['api repos/o/r', { default_branch: 'main' }],
    ['repo clone', ''],
  ];

// What a build in auto mode writes to its log first: a hook event, then its
// init event.
const startsInAuto = `echo '{"type":"system","subtype":"hook_started"}'
echo '{"type":"system","subtype":"init","permissionMode":"auto","model":"claude-opus-5-5"}'`;

// Fakes GitHub with `replies`, and puts beside the fake gh a fake claude that
// runs the shell commands `start`, then records its arguments and working
// directory in its log, then exits.
async function fakeBoth(replies, start = startsInAuto) {
  github = await fakeGithub(replies);
  const claude = join(process.env.PATH, 'claude');
  await writeFile(
    claude,
    `#!/bin/sh
${start}
printf '%s\\n' "$(pwd)"
for arg in "$@"; do printf '%s\\n' "$arg"; done
echo 'claude stderr' >&2
`,
  );
  await chmod(claude, 0o755);
}

// The lines the fake claude wrote to `log` after its init event, once it has
// written them.
async function claudeOutput(log) {
  for (let i = 0; i < 100; i++) {
    const text = await readFile(log, 'utf8').catch(() => '');
    if (text.includes('--settings')) {
      const lines = text.trim().split('\n');
      return lines.slice(lines.findIndex((line) => line.includes('"subtype":"init"')) + 1);
    }
    await sleep(20);
  }
  throw new Error(`fake claude never wrote ${log}`);
}

test('an issue without sub-issues is built with /epik:issue into the default branch', async () => {
  await fakeBoth(issueReplies());
  const result = await buildStart.run({ repo: 'o/r', number: 7 });

  assert.equal(result.kind, 'issue');
  assert.equal(result.command, '/epik:issue 7 main');
  assert.equal(result.branch, 'main');
  assert.match(result.run_id, /^o-r-7-\d{8}T\d{6}Z$/);
  assert.equal(result.clone, join(home, '.epik', 'builds', result.run_id));
  assert.equal(result.log, join(home, '.epik', 'runs', `${result.run_id}.jsonl`));
  assert.equal(result.errors_log, join(home, '.epik', 'runs', `${result.run_id}.err`));
  assert.ok(Number.isInteger(result.pid));

  const [cwd, ...args] = await claudeOutput(result.log);
  assert.equal(cwd, result.clone);
  assert.deepEqual(args, claudeArgs('/epik:issue 7 main', '/plugins/epik'));
  assert.equal(await readFile(result.errors_log, 'utf8'), 'claude stderr\n');

  const clone = (await github.calls()).find((call) => call[0] === 'repo');
  assert.deepEqual(clone, ['repo', 'clone', 'o/r', result.clone]);
});

test('an issue with sub-issues is built with /epik:epic on a branch named from its title', async () => {
  await fakeBoth([
    [pages('repos/o/r/issues/7/sub_issues'), [[{ number: 8 }]]],
    ['api repos/o/r/issues/7', issue()],
    ['repo clone', ''],
  ]);
  const result = await buildStart.run({ repo: 'o/r', number: 7 });

  assert.equal(result.kind, 'epic');
  assert.equal(result.command, '/epik:epic 7 epic/7-add-the-thing-part-2');
  const [, ...args] = await claudeOutput(result.log);
  assert.deepEqual(args, claudeArgs(result.command, '/plugins/epik'));
});

test('a given branch is used as is', async () => {
  await fakeBoth([
    [pages('repos/o/r/issues/7/sub_issues'), [[]]],
    ['api repos/o/r/issues/7', issue()],
    ['repo clone', ''],
  ]);
  const result = await buildStart.run({ repo: 'o/r', number: 7, branch: 'epic/9-x' });
  assert.equal(result.command, '/epik:issue 7 epic/9-x');
  await claudeOutput(result.log);
});

// Whether the process `pid` is running.
function running(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

test('a build that starts in auto mode after hook events is returned', async () => {
  await fakeBoth(
    issueReplies(),
    `echo 'not json'
echo '{"type":"system","subtype":"hook_started"}'
echo '{"type":"system","subtype":"hook_response"}'
echo '{"type":"system","subtype":"init","permissionMode":"auto","model":"m"}'`,
  );
  const result = await buildStart.run({ repo: 'o/r', number: 7 });
  assert.equal(result.command, '/epik:issue 7 main');
  await claudeOutput(result.log);
});

test('a build that starts in another permission mode is stopped', async () => {
  const pidFile = join(home, 'pid');
  await fakeBoth(
    issueReplies(),
    `echo $$ > '${pidFile}'
echo '{"type":"system","subtype":"hook_started"}'
echo '{"type":"system","subtype":"init","permissionMode":"default","model":"claude-sonnet-4-5"}'
/bin/sleep 30`,
  );
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), {
    message:
      'The build started in "default" permission mode, not "auto", so it couldn\'t run unattended, and it has been ' +
      'stopped. Its model was claude-sonnet-4-5. Auto mode needs a recent model (choose one with /model), and an ' +
      'organization admin can turn it off. See "Troubleshooting the install" in Epik\'s README.',
  });
  assert.equal(running(Number(await readFile(pidFile, 'utf8'))), false);
});

test('a build that exits before it starts fails with its errors', async () => {
  await fakeBoth(issueReplies(), `echo 'no auth' >&2
exit 1`);
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), { message: /exited before it started[^]*no auth/ });
});

test('a build that does not report its permission mode in time is stopped', async () => {
  const pidFile = join(home, 'pid');
  await fakeBoth(
    issueReplies(),
    `echo $$ > '${pidFile}'
echo '{"type":"system","subtype":"hook_started"}'
/bin/sleep 30`,
  );
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }, { initTimeoutMs: 300 }), {
    message: /didn't report its permission mode within 0\.3 seconds[^]*\.err/,
  });
  assert.equal(running(Number(await readFile(pidFile, 'utf8'))), false);
});

test('a closed issue is not built', async () => {
  await fakeBoth([['api repos/o/r/issues/7', issue('closed')]]);
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), { message: '#7 in o/r is closed.' });
});

test('a pull request is not built', async () => {
  await fakeBoth([['api repos/o/r/issues/7', issue('open', { pull_request: {} })]]);
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), { message: /is a pull request/ });
});

test('without CLAUDE_PLUGIN_ROOT nothing is built', async () => {
  delete process.env.CLAUDE_PLUGIN_ROOT;
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), { message: /CLAUDE_PLUGIN_ROOT is not set/ });
});

test('without Claude Code the build fails saying so', async () => {
  github = await fakeGithub(issueReplies());
  await assert.rejects(buildStart.run({ repo: 'o/r', number: 7 }), { message: /Claude Code is not installed/ });
});

test('a slug is the title lowercased, dashed, cut to 40 characters and trimmed', () => {
  assert.equal(slug('  Hello, World!  '), 'hello-world');
  assert.equal(slug('Ünïcode & émoji 🎉 ok'), 'n-code-moji-ok');
  assert.equal(slug('a'.repeat(39) + ' b'), 'a'.repeat(39));
  assert.equal(slug('!!!'), '');
});

test('an epic branch without a slug is just its number', () => {
  assert.equal(epicBranch(7, 'Big plan'), 'epic/7-big-plan');
  assert.equal(epicBranch(7, '!!!'), 'epic/7');
});

test('a stamp is the UTC time to the second', () => {
  assert.equal(stamp(new Date('2026-10-26T21:05:09.123Z')), '20261026T210509Z');
});

test('a build loads only the plugin copy at the given root, in auto mode without prompts', () => {
  assert.deepEqual(claudeArgs('/epik:issue 1 main', '/p'), [
    '-p', '/epik:issue 1 main',
    '--output-format', 'stream-json', '--verbose', '--forward-subagent-text',
    '--permission-mode', 'auto', '--permission-prompts', 'none',
    '--plugin-dir', '/p',
    '--settings', '{"enabledPlugins":{"epik@epik":false}}',
  ]);
});
