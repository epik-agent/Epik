// Levels in a dependency graph.

// Levels the graph `blockers`, a map from each issue to the issues blocking
// it. An issue's position is 0 with no blockers, else one more than its
// deepest blocker's. Issues on a cycle, or blocked by one, have position null.
// `cycles` lists each cycle's issues, each cycle and the list ordered by
// `compare`.
//
// Tarjan's algorithm finds the strongly connected components, emitting each
// after every component it can reach, so an issue's blockers are leveled
// before it is.
export function levels(blockers, compare = (a, b) => a - b) {
  const positions = new Map();
  const cycles = [];
  const index = new Map();
  const low = new Map();
  const stack = [];
  const onStack = new Set();

  const visit = (issue) => {
    index.set(issue, index.size);
    low.set(issue, index.get(issue));
    stack.push(issue);
    onStack.add(issue);
    for (const blocker of blockers.get(issue)) {
      if (!index.has(blocker)) {
        visit(blocker);
        low.set(issue, Math.min(low.get(issue), low.get(blocker)));
      } else if (onStack.has(blocker)) {
        low.set(issue, Math.min(low.get(issue), index.get(blocker)));
      }
    }
    if (low.get(issue) !== index.get(issue)) return;
    const component = stack.splice(stack.indexOf(issue));
    for (const member of component) onStack.delete(member);
    if (component.length > 1 || blockers.get(issue).includes(issue)) {
      cycles.push(component.sort(compare));
      for (const member of component) positions.set(member, null);
    } else {
      const below = blockers.get(issue).map((blocker) => positions.get(blocker));
      positions.set(issue, below.includes(null) ? null : 1 + Math.max(-1, ...below));
    }
  };

  for (const issue of blockers.keys()) if (!index.has(issue)) visit(issue);
  return { positions, cycles: cycles.sort((a, b) => compare(a[0], b[0])) };
}
