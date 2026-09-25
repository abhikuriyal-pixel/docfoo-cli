import test from 'node:test';
import assert from 'node:assert/strict';

import {
  buildHighlights,
  cadenceFor,
  expansionBanner,
  retryBanner,
  routingBanner,
  stripForDelivered,
  stripForReorder,
} from '../../src/kg/vis/assets/js/choreo.js';
import { createVizState, applyStageFrame } from '../../src/kg/vis/assets/js/state.js';

const graph = {
  hash: 'test',
  nodes: [
    ['a', 'A', 'CONCEPT', 2],
    ['b', 'B', 'CONCEPT', 1],
    ['c', 'C', 'CONCEPT', 1],
    ['d', 'D', 'CONCEPT', 0],
  ],
  edges: [[0, 1], [1, 2], [0, 2]],
  sections: {},
  noiseFloor: 0,
};

const layout = {
  points: [
    { x: 0, y: 0, r: 4 },
    { x: 10, y: 0, r: 3 },
    { x: 20, y: 0, r: 3 },
    { x: 30, y: 0, r: 3 },
  ],
  clusterOf: [0, 0, 0, -1],
  centers: [{ x: 0, y: 0, mass: 4 }],
  hubs: [0],
  width: 40,
  height: 10,
  degenerate: false,
};

test('highlight specs preserve directed frontier edge order', () => {
  let state = createVizState();
  state = applyStageFrame(state, { type: 'stage', pass: 1, seq: 1, step: 'seeds', data: { ids: ['a'], anchors: ['a'], leadIds: [] } });
  state = applyStageFrame(state, { type: 'stage', pass: 1, seq: 2, step: 'hop', data: { hop: 1, added: ['b', 'c'] } });

  const spec = buildHighlights(state, graph, layout);
  assert.deepEqual([...spec.seeds], [0]);
  assert.deepEqual([...spec.levelAdded[0]].sort(), [1, 2]);
  // Both traversal edges cross from the known seed into the fresh level.
  assert.deepEqual(spec.levelEdges[0], [[0, 1], [0, 2]]);
  assert.equal(spec.guideClusters.size, 0, 'no guides → no cluster outlines');
});

test('banners and the evidence strip carry the desktop wording', () => {
  const expansion = expansionBanner({ escalationTerms: ['axial', 'cross'] });
  assert.match(expansion.msg, /Weak seeds/);
  assert.deepEqual(expansion.chips, ['axial', 'cross']);

  const routing = routingBanner({ routingUsed: true, routePicks: [{ name: 'Attention', prob: 0.8 }] });
  assert.match(routing.msg, /Concept routing - 1 matched/);
  assert.deepEqual(routing.chips, ['Attention 80%']);

  const retry = retryBanner({ retryActive: true, avoidTerms: ['math'] });
  assert.match(retry.msg, /Pass refused/);
  assert.deepEqual(retry.chips, ['math']);

  const provisional = stripForDelivered(['S1', 'S2'], 5, ['S2']);
  assert.equal(provisional.provisional, true);
  assert.equal(provisional.hiddenMore, 3);
  assert.equal(provisional.rows[1].direct, true);

  const { view, movedRows } = stripForReorder(
    ['S1', 'S2'],
    [{ section: 'S2', chars: 90 }, { section: 'S1', chars: 50 }],
    3,
    ['S9'],
    1,
    [],
  );
  assert.deepEqual(view.rows.map((row) => row.title), ['S2', 'S1', 'S9']);
  assert.equal(view.rows[2].dropped, true);
  assert.equal(view.hiddenMore, 1);
  assert.ok(movedRows.has(0) && movedRows.has(1), 'reordered rows flash');
});

test('replay cadence stays watchable and bounded', () => {
  assert.equal(cadenceFor(0), 380);
  assert.equal(cadenceFor(1), 380);
  assert.equal(cadenceFor(100), 150);
  assert.ok(cadenceFor(10_000) >= 45);
});
