import assert from 'node:assert/strict';
import { test } from 'node:test';

import { levels } from '../src/graph.js';

const graph = (edges) => new Map(Object.entries(edges).map(([issue, blockers]) => [Number(issue), blockers]));

test('a chain has one level per link', () => {
  assert.deepEqual(levels(graph({ 1: [], 2: [1], 3: [2] })), {
    positions: new Map([
      [1, 0],
      [2, 1],
      [3, 2],
    ]),
    cycles: [],
  });
});

test('a diamond sits one level above its deepest blocker', () => {
  assert.deepEqual(
    levels(graph({ 4: [2, 3], 3: [1], 2: [1], 1: [] })).positions,
    new Map([
      [1, 0],
      [2, 1],
      [3, 1],
      [4, 2],
    ]),
  );
});

test('issues on a cycle, and those waiting on one, have no level', () => {
  assert.deepEqual(levels(graph({ 1: [2], 2: [1], 3: [1], 4: [], 5: [5] })), {
    positions: new Map([
      [1, null],
      [2, null],
      [3, null],
      [4, 0],
      [5, null],
    ]),
    cycles: [[1, 2], [5]],
  });
});

test('cycles are ordered by the compare function given', () => {
  const blockers = new Map([
    ['b', ['a']],
    ['a', ['b']],
  ]);
  assert.deepEqual(levels(blockers, (x, y) => x.localeCompare(y)).cycles, [['a', 'b']]);
});
