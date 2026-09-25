import test from 'node:test';
import assert from 'node:assert/strict';

import { computeLayout } from '../../src/kg/vis/assets/js/layout.js';

/** Build a projection helper in the wire shape the server ships. */
function projection(nodes, edges) {
  return {
    hash: 'test',
    nodes: nodes.map(([id, degree]) => [id, id, 'CONCEPT', degree]),
    edges,
    sections: {},
    noiseFloor: 0,
    descs: nodes.map(() => ''),
  };
}

/** Two dense communities bridged by one edge. */
function twoCommunities() {
  const nodes = [];
  const edges = [];
  for (let i = 0; i < 12; i += 1) nodes.push([`a${i}`, 3]);
  for (let i = 0; i < 12; i += 1) nodes.push([`b${i}`, 3]);
  for (let i = 0; i < 10; i += 1) {
    edges.push([i, i + 1]);
    edges.push([12 + i, 12 + i + 1]);
  }
  edges.push([5, 17]); // bridge
  return projection(nodes, edges);
}

test('layout is deterministic and normalized into the positive quadrant', () => {
  const graph = twoCommunities();
  const first = computeLayout(graph);
  const second = computeLayout(graph);

  assert.deepEqual(JSON.parse(JSON.stringify(first)), JSON.parse(JSON.stringify(second)));
  assert.equal(first.points.length, graph.nodes.length);

  for (const point of first.points) {
    assert.ok(Number.isFinite(point.x) && Number.isFinite(point.y));
    assert.ok(point.x - point.r >= -1e-6, 'x inside the padded box');
    assert.ok(point.y - point.r >= -1e-6, 'y inside the padded box');
    assert.ok(point.x + point.r <= first.width + 1e-6);
    assert.ok(point.y + point.r <= first.height + 1e-6);
    assert.ok(point.r >= 2.5 && point.r <= 12);
  }
});

test('community members never overlap and hubs sit on their centers', () => {
  const graph = twoCommunities();
  const layout = computeLayout(graph);

  assert.equal(layout.degenerate, false);
  assert.ok(layout.centers.length >= 2);

  for (let i = 0; i < layout.points.length; i += 1) {
    for (let j = i + 1; j < layout.points.length; j += 1) {
      if (layout.clusterOf[i] !== layout.clusterOf[j]) continue;
      const a = layout.points[i];
      const b = layout.points[j];
      const distance = Math.hypot(a.x - b.x, a.y - b.y);
      assert.ok(distance >= a.r + b.r - 1e-6, `nodes ${i}/${j} overlap`);
    }
  }

  layout.hubs.forEach((hub, cluster) => {
    const center = layout.centers[cluster];
    const point = layout.points[hub];
    assert.ok(Math.hypot(point.x - center.x, point.y - center.y) < 1e-6, 'hub pinned at center');
  });
});

test('sparse and disconnected graphs take the concentric fallback', () => {
  const graph = projection(
    [['a', 0], ['b', 0], ['c', 0], ['d', 0], ['hub', 4]],
    [[0, 1], [1, 2], [2, 3], [3, 4]],
  );
  const layout = computeLayout(graph);
  assert.equal(layout.degenerate, true);
  assert.equal(layout.points.length, 5);
});

test('empty and malformed graphs never throw', () => {
  assert.equal(computeLayout(projection([], [])).points.length, 0);
  const selfLoops = computeLayout(projection([['a', 1], ['b', 0]], [[0, 0], [1, 0]]));
  assert.equal(selfLoops.points.length, 2);
});
