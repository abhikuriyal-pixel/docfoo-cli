import test from 'node:test';
import assert from 'node:assert/strict';

import {
  applyStageFrame,
  computeStateAt,
  createVizState,
  parseStageFrame,
  takeCapture,
  vizApplyFrame,
  vizCurrent,
  vizReset,
} from '../../src/kg/vis/assets/js/state.js';

const frame = (step, data = {}, seq = 1) => ({ type: 'stage', pass: 1, seq, step, data });

test('wire parser is tolerant and forward compatible', () => {
  assert.equal(parseStageFrame(null), null);
  assert.equal(parseStageFrame({ type: 'delta' }), null);
  assert.equal(parseStageFrame({ type: 'stage', step: '' }), null);

  const parsed = parseStageFrame({ type: 'stage', step: 'futureStep', pass: 2, seq: 7, data: { x: 1 } });
  assert.deepEqual(parsed, { type: 'stage', pass: 2, seq: 7, step: 'futureStep', data: { x: 1 } });

  // Missing data / pass / seq default instead of rejecting.
  const loose = parseStageFrame({ type: 'stage', step: 'seeds' });
  assert.deepEqual(loose.data, {});
  assert.equal(loose.pass, 1);
  assert.equal(loose.seq, 0);
});

test('reducer is pure and accumulates traversal + evidence state', () => {
  const before = createVizState();
  const snapshot = JSON.stringify(before, (key, value) => (value instanceof Set ? [...value] : value));

  let state = applyStageFrame(before, frame('seeds', { ids: ['a', 'b'], anchors: ['a'], leadIds: ['b'] }));
  state = applyStageFrame(state, frame('hop', { hop: 1, added: ['c'] }, 2));
  state = applyStageFrame(state, frame('hop', { hop: 2, added: ['d', 'unknown'] }, 3));
  state = applyStageFrame(state, frame('traversal', { visitedCount: 4 }, 4));
  state = applyStageFrame(state, frame('delivered', { tierOrder: ['S1', 'S2'], tierTotal: 2 }, 5));
  state = applyStageFrame(state, frame('reorder', {
    finalOrder: [{ section: 'S2', chars: 120 }, { section: 'S1', chars: 80 }],
    finalTotal: 2,
    dropped: ['S3'],
    droppedTotal: 1,
  }, 6));
  state = applyStageFrame(state, frame('evidenceSummary', { triples: 17 }, 7));

  assert.deepEqual(state.seeds, ['a', 'b']);
  assert.deepEqual(state.anchors, ['a']);
  assert.deepEqual(state.leads, ['b']);
  assert.equal(state.hops.length, 2);
  assert.deepEqual([...state.visited].sort(), ['c', 'd', 'unknown']);
  assert.deepEqual(state.finalOrder.map((row) => row.section), ['S2', 'S1']);
  assert.deepEqual(state.droppedSections, ['S3']);
  assert.equal(state.triples, 17);
  assert.equal(state.finished, false);

  // Input was not mutated.
  const after = JSON.stringify(before, (key, value) => (value instanceof Set ? [...value] : value));
  assert.equal(after, snapshot);
});

test('retry resets the pass but keeps the avoidance hint; unknown seeds are filtered', () => {
  let state = applyStageFrame(createVizState(), frame('seeds', { ids: ['a'], anchors: ['missing'], leadIds: ['a'] }));
  assert.deepEqual(state.anchors, [], 'anchors must be a subset of the seeds');

  state = applyStageFrame(state, frame('retry', { avoidTerms: ['math'] }, 9));
  assert.equal(state.retryActive, true);
  assert.equal(state.pass, 2);
  assert.deepEqual(state.avoidTerms, ['math']);
  assert.deepEqual(state.seeds, []);
  assert.deepEqual(state.hops, []);

  state = applyStageFrame(state, frame('done', { totalSecs: 1 }, 10));
  assert.equal(state.finished, true);
});

test('computeStateAt folds recorded frames identically, and capture swaps out', () => {
  vizReset();
  const frames = [
    parseStageFrame(frame('seeds', { ids: ['a'], anchors: [], leadIds: [] }, 1)),
    parseStageFrame(frame('hop', { hop: 1, added: ['b'] }, 2)),
  ];
  vizApplyFrame(frames[0]);
  vizApplyFrame(frames[1]);

  const folded = computeStateAt(frames, 2);
  assert.deepEqual(folded.seeds, ['a']);
  assert.deepEqual(folded.hops, [{ hop: 1, added: ['b'] }]);

  const captured = takeCapture();
  assert.equal(captured.length, 2);
  assert.equal(takeCapture().length, 0, 'capture hands over exactly once');
  assert.deepEqual(vizCurrent().seeds, ['a']);
});
