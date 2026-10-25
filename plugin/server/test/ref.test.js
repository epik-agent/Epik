import assert from 'node:assert/strict';
import { test } from 'node:test';

import { byUrl, issueUrl, pullUrl, repoOf, sameRepo } from '../src/ref.js';

test('an issue URL names its repository and number, the repository lowercased', () => {
  assert.equal(issueUrl('Epik-Agent/Epik', 305), 'https://github.com/epik-agent/epik/issues/305');
});

test('a pull request URL names its repository and number, the repository lowercased', () => {
  assert.equal(pullUrl('Epik-Agent/Epik', 322), 'https://github.com/epik-agent/epik/pull/322');
});

test('the same issue in two spellings of its repository has one URL', () => {
  assert.equal(issueUrl('o/R', 7), issueUrl('O/r', 7));
  assert.notEqual(issueUrl('o/r', 7), issueUrl('other/lib', 7));
});

test('repoOf reads owner/name from a REST issue or pull request', () => {
  assert.equal(repoOf({ repository_url: 'https://api.github.com/repos/other/lib' }), 'other/lib');
});

test('sameRepo ignores case', () => {
  assert.ok(sameRepo('Epik-Agent/Epik', 'epik-agent/epik'));
  assert.ok(!sameRepo('o/r', 'o/s'));
});

test('byUrl orders URLs by repository, then by number', () => {
  const urls = [issueUrl('o/r', 10), issueUrl('o/r', 9), issueUrl('a/b', 100)];
  assert.deepEqual(urls.sort(byUrl), [issueUrl('a/b', 100), issueUrl('o/r', 9), issueUrl('o/r', 10)]);
});
