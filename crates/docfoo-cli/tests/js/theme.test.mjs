import test from 'node:test';
import assert from 'node:assert/strict';

import {
  normalizeTheme,
  readTheme,
  toggleTheme,
  writeTheme,
} from '../../src/kg/vis/assets/js/theme.js';

test('art-deco is the launch default', () => {
  assert.equal(normalizeTheme(undefined), 'art-deco');
  assert.equal(normalizeTheme('nonsense'), 'art-deco');
  assert.equal(normalizeTheme('kinetic'), 'kinetic');
  assert.equal(readTheme({ getItem: () => null }), 'art-deco');
  assert.equal(readTheme({ getItem: () => 'kinetic' }), 'kinetic');
  assert.equal(toggleTheme('art-deco'), 'kinetic');
  assert.equal(toggleTheme('kinetic'), 'art-deco');
});

test('writeTheme persists a normalized value', () => {
  const store = new Map();
  const storage = { setItem: (key, value) => store.set(key, value) };
  assert.equal(writeTheme(storage, 'kinetic'), 'kinetic');
  assert.equal(store.get('docfoo-kg-vis-theme'), 'kinetic');
  assert.equal(writeTheme(storage, 'bogus'), 'art-deco');
  assert.equal(store.get('docfoo-kg-vis-theme'), 'art-deco');
});
