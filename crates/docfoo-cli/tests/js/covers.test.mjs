import test from 'node:test';
import assert from 'node:assert/strict';

import {
  MAX_COLLAGE,
  cardMeta,
  coverPlan,
  coverSignature,
  createCoverPicker,
  formatBytes,
  kindLabel,
  randomOf,
  shuffled,
} from '../../src/resources/vis/assets/js/covers.js';

/** Deterministic rng cycling through the given values. */
function seeded(values) {
  let index = 0;
  return () => values[index++ % values.length];
}

const entry = (over = {}) => ({
  name: 'Book',
  rel: 'Book',
  kind: 'dir',
  size: 0,
  files: 3,
  md: 1,
  figures: 2,
  covers: [],
  ...over,
});

test('folders pick a collage of up to four distinct figures', () => {
  const covers = ['a.png', 'b.png', 'c.png', 'd.png', 'e.png'].map((name) => `Book/assets/${name}`);
  const plan = coverPlan(entry({ covers }), seeded([0.9, 0.1, 0.5, 0.3, 0.7, 0.2, 0.8, 0.4]));
  assert.equal(plan.shape, 'collage');
  assert.equal(plan.figures.length, MAX_COLLAGE);
  assert.equal(new Set(plan.figures).size, MAX_COLLAGE);
  for (const figure of plan.figures) assert.ok(covers.includes(figure));
});

test('a single cover stays a figure and a file picks one random figure', () => {
  const single = coverPlan(entry({ covers: ['Book/assets/only.png'] }));
  assert.equal(single.shape, 'figure');
  assert.deepEqual(single.figures, ['Book/assets/only.png']);

  const file = coverPlan(
    entry({ kind: 'md', covers: ['Book/assets/x.png', 'Book/assets/y.png'] }),
    seeded([0.99]),
  );
  assert.equal(file.shape, 'figure');
  assert.equal(file.figures.length, 1);
  assert.ok(['Book/assets/x.png', 'Book/assets/y.png'].includes(file.figures[0]));
});

test('figure-less entries fall back to a monogram or a kind glyph', () => {
  assert.deepEqual(coverPlan(entry()), { shape: 'mark', figures: [] });
  const glyph = coverPlan(entry({ kind: 'md' }));
  assert.equal(glyph.shape, 'glyph');
  assert.equal(glyph.kind, 'md');
});

test('the picker caches covers until the dice re-rolls', () => {
  const covers = Array.from({ length: 8 }, (_, index) => `Book/assets/f${index}.png`);
  const picker = createCoverPicker(seeded([0.11, 0.22, 0.33, 0.44, 0.55, 0.66, 0.77, 0.88]));
  const card = entry({ covers });
  const first = picker.plan(card);
  assert.equal(picker.plan(card), first);
  assert.ok(coverSignature(first).startsWith(first.shape));

  picker.reroll();
  const second = picker.plan(card);
  assert.notEqual(coverSignature(second), coverSignature(first));
});

test('shuffled returns a copy and randomOf never misses', () => {
  const source = [1, 2, 3, 4];
  const copy = shuffled(source, seeded([0.5]));
  assert.deepEqual(source, [1, 2, 3, 4]);
  assert.deepEqual([...copy].sort(), [1, 2, 3, 4]);
  assert.equal(randomOf([], seeded([0.5])), null);
  assert.equal(randomOf(['a'], seeded([0.5])), 'a');
  assert.equal(randomOf(['a', 'b'], seeded([1])), 'b');
});

test('meta lines and labels read like the desktop browser', () => {
  assert.equal(cardMeta(entry()), '3 files · 1 doc · 2 figures');
  assert.equal(cardMeta(entry({ kind: 'md', files: null, md: null, figures: null, lines: 120, size: 2048 })), '2 KB · 120 lines');
  assert.equal(formatBytes(0), '0 B');
  assert.equal(formatBytes(1536), '2 KB');
  assert.equal(formatBytes(5 * 1024 * 1024), '5.0 MB');
  assert.equal(kindLabel('dir'), 'Folder');
  assert.equal(kindLabel('md'), 'Markdown');
  assert.equal(kindLabel('image'), 'Image');
  assert.equal(kindLabel('pdf'), 'File');
});
