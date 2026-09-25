import test from 'node:test';
import assert from 'node:assert/strict';

import {
  buildNote,
  newNoteId,
  noteLine,
  sortNotes,
} from '../../src/resources/vis/assets/js/anchor.js';

test('note ids are unique and readable', () => {
  const first = newNoteId(1_700_000_000_000, () => 0.5);
  const second = newNoteId(1_700_000_000_000, () => 0.25);
  assert.match(first, /^ann-[0-9a-z]+-[0-9a-z]+$/);
  assert.notEqual(first, second);
});

test('buildNote follows the desktop note schema', () => {
  const note = buildNote({
    id: 'ann-1',
    text: 'Great point',
    quote: 'selected words',
    line: 12,
    start: 3,
    end: 17,
    created: 1_700_000_000_000,
  });
  assert.deepEqual(note, {
    id: 'ann-1',
    type: 'NOTE',
    text: 'Great point',
    originalText: 'selected words',
    filePath: '',
    created: 1_700_000_000_000,
    anchor: { startLine: 11, startCol: 3, endLine: 11, endCol: 17 },
    sourceLine: 12,
  });
});

test('notes sort by document position, then creation time', () => {
  const sorted = sortNotes([
    { id: 'c', sourceLine: 20, created: 1 },
    { id: 'a', sourceLine: 3, created: 9 },
    { id: 'b', sourceLine: 3, created: 2 },
    { id: 'd', created: 5 },
  ]);
  assert.deepEqual(sorted.map((note) => note.id), ['b', 'a', 'c', 'd']);
});

test('noteLine falls back to the stored anchor', () => {
  assert.equal(noteLine({ sourceLine: 7 }), 7);
  assert.equal(noteLine({ anchor: { startLine: 4 } }), 5);
  assert.equal(noteLine({}), 0);
  assert.equal(noteLine(undefined), 0);
});
