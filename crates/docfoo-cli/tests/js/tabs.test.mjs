import test from 'node:test';
import assert from 'node:assert/strict';

import {
  TABS_STORAGE_KEY,
  addTab,
  loadTabs,
  neighborRel,
  removeTab,
  saveTabs,
  tabLabels,
} from '../../src/resources/vis/assets/js/tabs.js';

const doc = (rel, name) => ({ rel, name });

function memoryStorage() {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, value),
  };
}

test('tabs open once and keep their latest name', () => {
  let tabs = [];
  tabs = addTab(tabs, doc('Book/content.md', 'content.md'));
  tabs = addTab(tabs, doc('Other/content.md', 'content.md'));
  assert.equal(tabs.length, 2);
  tabs = addTab(tabs, doc('Book/content.md', 'renamed.md'));
  assert.equal(tabs.length, 2);
  assert.equal(tabs.find((tab) => tab.rel === 'Book/content.md').name, 'renamed.md');
});

test('closing activates the next tab, then the previous', () => {
  const tabs = [doc('a', 'a'), doc('b', 'b'), doc('c', 'c')];
  assert.equal(neighborRel(tabs, 'a'), 'b');
  assert.equal(neighborRel(tabs, 'b'), 'c');
  assert.equal(neighborRel(tabs, 'c'), 'b');
  assert.equal(neighborRel(tabs, 'missing'), null);
  assert.deepEqual(removeTab(tabs, 'b').map((tab) => tab.rel), ['a', 'c']);
});

test('duplicate names get a folder prefix', () => {
  const tabs = [doc('Book/content.md', 'content.md'), doc('Notes/content.md', 'content.md'), doc('Solo/guide.md', 'guide.md')];
  assert.deepEqual(tabLabels(tabs), ['Book/content.md', 'Notes/content.md', 'guide.md']);
});

test('persistence round-trips through storage and tolerates garbage', () => {
  const storage = memoryStorage();
  saveTabs(storage, [doc('a/content.md', 'content.md')], 'a/content.md');
  assert.deepEqual(loadTabs(storage), {
    tabs: [{ rel: 'a/content.md', name: 'content.md' }],
    active: 'a/content.md',
  });

  storage.setItem(TABS_STORAGE_KEY, 'not json');
  assert.deepEqual(loadTabs(storage), { tabs: [], active: null });

  storage.setItem(TABS_STORAGE_KEY, JSON.stringify({ tabs: [{ rel: 'ok.md' }, { nope: true }], active: 'missing' }));
  assert.deepEqual(loadTabs(storage), { tabs: [{ rel: 'ok.md' }], active: null });

  assert.deepEqual(loadTabs(undefined), { tabs: [], active: null });
});
