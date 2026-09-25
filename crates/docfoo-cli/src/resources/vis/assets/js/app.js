/**
 * Resource browser controller: library cards, document tabs, the reader,
 * outline, notes and the theme/typography controls.
 *
 * The page is read-only with respect to resource files; notes are the only
 * thing it writes, and they go to the desktop app's `notes.json`.
 */

import * as api from './api.js';
import { DICE_ICON, levelDescription, levelTitle, renderCrumbs, renderGrid } from './browser.js';
import { createCoverPicker } from './covers.js';
import { createNoteController } from './notes.js';
import { blockForLine, renderDocument, renderToc, watchOutline } from './reader.js';
import { addTab, loadTabs, neighborRel, removeTab, saveTabs, tabLabels } from './tabs.js';
import { applyTheme, readTheme, toggleTheme, writeTheme } from './theme.js';

const FONT_KEY = 'docfoo-res-vis-font';
const FIGURE_KEY = 'docfoo-res-vis-figure';
const FONT_DEFAULT = 20;
const FIGURE_DEFAULT = 65;
const FONT_RANGE = [12, 45];
const FIGURE_RANGE = [25, 100];

const el = (id) => document.getElementById(id);
const dom = {
  theme: el('theme-toggle'),
  dice: el('dice'),
  status: el('res-status'),
  browser: el('browser-pane'),
  viewer: el('viewer-pane'),
  crumbs: el('crumbs'),
  levelTitle: el('level-title'),
  levelDesc: el('level-desc'),
  grid: el('grid'),
  empty: el('empty'),
  browserError: el('browser-error'),
  tabs: el('tabs'),
  docTitle: el('doc-title'),
  docError: el('doc-error'),
  tocToggle: el('toc-toggle'),
  notesToggle: el('notes-toggle'),
  fontSlider: el('font-slider'),
  figureSlider: el('figure-slider'),
  toc: el('toc'),
  tocList: el('toc-list'),
  tocCount: el('toc-count'),
  docBody: el('doc-body'),
  docMd: el('doc-md'),
  notesPanel: el('notes-panel'),
  notesList: el('notes-list'),
  notesCount: el('notes-count'),
  notesEmpty: el('notes-empty'),
  toolbar: el('annotation-toolbar'),
  annotate: el('annotate-note'),
  popover: el('note-popover'),
  popoverTitle: el('popover-title'),
  popoverQuote: el('popover-quote'),
  popoverError: el('popover-error'),
  popoverClose: el('popover-close'),
  noteText: el('note-text'),
  noteCancel: el('note-cancel'),
  noteSave: el('note-save'),
  lightbox: el('lightbox'),
  lightboxImg: el('lightbox-img'),
};

function storageGet(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function storageSet(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private browsing / storage disabled: the session still applies it.
  }
}

function setStatus(message) {
  dom.status.textContent = message;
  clearTimeout(setStatus.timer);
  if (message) setStatus.timer = setTimeout(() => { dom.status.textContent = ''; }, 2600);
}

const state = {
  rel: '',
  startRel: '',
  entries: [],
  docs: [],
  active: null,
  outline: [],
  unwatchOutline: null,
};

const picker = createCoverPicker();

const noteController = createNoteController({
  body: dom.docMd,
  toolbar: dom.toolbar,
  popover: dom.popover,
  popoverTitle: dom.popoverTitle,
  popoverQuote: dom.popoverQuote,
  popoverError: dom.popoverError,
  textarea: dom.noteText,
  list: dom.notesList,
  count: dom.notesCount,
  empty: dom.notesEmpty,
  notify: setStatus,
  api: { saveNote: api.saveNote, deleteNote: api.deleteNote },
});

// ── Theme ───────────────────────────────────────────────────────────────────

function updateTheme() {
  const theme = readTheme(localStorage);
  applyTheme(theme);
  dom.theme.textContent = theme === 'kinetic' ? 'Kinetic' : 'Art Deco';
}

dom.theme.addEventListener('click', () => {
  writeTheme(localStorage, toggleTheme(readTheme(localStorage)));
  updateTheme();
  dom.theme.classList.remove('bump');
  void dom.theme.offsetWidth;
  dom.theme.classList.add('bump');
});

// ── Typography sliders ──────────────────────────────────────────────────────

function readSetting(key, fallback, [min, max]) {
  const value = Number(storageGet(key));
  return Number.isFinite(value) && value >= min && value <= max ? Math.round(value) : fallback;
}

function applyTypography() {
  document.documentElement.style.setProperty('--reader-font-size', `${dom.fontSlider.value}px`);
  document.documentElement.style.setProperty('--res-figure-scale', String(Number(dom.figureSlider.value) / 100));
}

dom.fontSlider.addEventListener('input', () => {
  storageSet(FONT_KEY, dom.fontSlider.value);
  applyTypography();
});

dom.figureSlider.addEventListener('input', () => {
  storageSet(FIGURE_KEY, dom.figureSlider.value);
  applyTypography();
});

// ── Library ─────────────────────────────────────────────────────────────────

/** Folder of a resource rel path: "Book/content.md" → "Book". */
function parentRel(rel) {
  const parts = String(rel ?? '').split('/').filter(Boolean);
  parts.pop();
  return parts.join('/');
}

async function loadLevel(rel) {
  state.rel = rel;
  state.active = null;
  dom.viewer.hidden = true;
  dom.browser.hidden = false;
  dom.browserError.hidden = true;
  try {
    const level = await api.getLevel(rel);
    const normalized = level.rel ?? rel;
    state.entries = level.entries;
    renderCrumbs(dom.crumbs, normalized, {
      onNavigate: loadLevel,
      onBack: () => loadLevel(parentRel(normalized)),
      canGoBack: normalized !== '',
    });
    dom.levelTitle.textContent = levelTitle(level.rel ?? rel);
    dom.levelDesc.textContent = levelDescription(level.entries);
    renderGrid(dom.grid, level.entries, picker, { onOpen: openEntry });
    dom.empty.hidden = level.entries.length > 0;
    dom.empty.textContent = rel
      ? 'This folder is empty.'
      : 'No resources yet — add markdown under resources/ and refresh.';
  } catch (error) {
    state.entries = [];
    dom.grid.textContent = '';
    dom.empty.hidden = true;
    dom.browserError.hidden = false;
    dom.browserError.textContent = error.message;
  }
}

function openEntry(entry) {
  if (entry.kind === 'dir') {
    loadLevel(entry.rel);
    return;
  }
  if (entry.kind === 'md') {
    openDoc(entry.rel, entry.name);
    return;
  }
  if (entry.kind === 'image') {
    openLightbox(`/api/asset?path=${encodeURIComponent(entry.rel)}`);
  }
}

function rerenderGrid() {
  renderGrid(dom.grid, state.entries, picker, { onOpen: openEntry });
}

dom.dice.innerHTML = DICE_ICON;
dom.dice.addEventListener('click', () => {
  picker.reroll();
  dom.dice.classList.remove('spinning');
  void dom.dice.offsetWidth;
  dom.dice.classList.add('spinning');
  if (!dom.browser.hidden) rerenderGrid();
});

// ── Document tabs and reader ────────────────────────────────────────────────

function persistTabs() {
  saveTabs(localStorage, state.docs, state.active);
}

function renderTabs() {
  dom.tabs.textContent = '';
  dom.tabs.hidden = state.docs.length === 0;
  const labels = tabLabels(state.docs);
  state.docs.forEach((doc, index) => {
    const tab = document.createElement('div');
    tab.className = `res-tab${doc.rel === state.active ? ' active' : ''}`;

    const label = document.createElement('button');
    label.type = 'button';
    label.className = 'res-tab-label';
    label.textContent = labels[index];
    label.title = doc.rel;
    label.addEventListener('click', () => activateDoc(doc.rel));

    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'res-tab-close';
    close.textContent = '×';
    close.title = 'Close';
    close.setAttribute('aria-label', `Close ${doc.name}`);
    close.addEventListener('click', (event) => {
      event.stopPropagation();
      closeDoc(doc.rel);
    });

    tab.addEventListener('auxclick', (event) => {
      if (event.button === 1) {
        event.preventDefault();
        closeDoc(doc.rel);
      }
    });
    tab.append(label, close);
    dom.tabs.appendChild(tab);
  });
}

function showViewer() {
  dom.browser.hidden = true;
  dom.viewer.hidden = false;
}

function showBrowser() {
  state.active = null;
  dom.viewer.hidden = true;
  dom.browser.hidden = false;
  renderTabs();
}

async function activateDoc(rel) {
  if (state.active === rel && !dom.viewer.hidden) return;
  const doc = state.docs.find((candidate) => candidate.rel === rel);
  await openDoc(rel, doc?.name);
}

async function openDoc(rel, name) {
  const doc = { rel, name: name || rel.split('/').pop() };
  state.docs = addTab(state.docs, doc);
  state.active = rel;
  persistTabs();
  renderTabs();
  showViewer();

  dom.docTitle.textContent = doc.name;
  dom.docError.hidden = true;
  dom.docMd.textContent = '';
  dom.toc.hidden = true;
  dom.notesPanel.hidden = true;
  dom.docBody.scrollTop = 0;
  noteController.setResource(rel);

  const folder = parentRel(rel);
  renderCrumbs(dom.crumbs, folder, {
    onNavigate: loadLevel,
    onBack: () => loadLevel(parentRel(folder)),
    canGoBack: true,
  });

  try {
    const [resource, noteData] = await Promise.all([api.getResource(rel), api.getNotes(rel)]);
    if (state.active !== rel) return;
    const baseDir = resource.rel.split('/').slice(0, -1).join('/');
    const rendered = renderDocument(dom.docMd, resource.text, baseDir);
    renderOutline(rendered.outline);
    noteController.setNotes(noteData.notes);
  } catch (error) {
    dom.docError.hidden = false;
    dom.docError.textContent = error.message;
    state.outline = [];
    dom.tocList.textContent = '';
    dom.tocCount.textContent = '';
  }
}

function renderOutline(outline) {
  state.outline = outline;
  const items = renderToc(dom.tocList, state.outline, (line) => {
    blockForLine(dom.docMd, line)?.scrollIntoView({ block: 'start', behavior: 'smooth' });
  });
  dom.tocCount.textContent = state.outline.length > 0 ? ` (${state.outline.length})` : '';
  state.unwatchOutline?.();
  state.unwatchOutline = watchOutline(dom.docBody, dom.docMd, state.outline, (line) => {
    for (const item of items) item.classList.toggle('active', Number(item.dataset.line) === line);
  });
}

function closeDoc(rel) {
  const next = neighborRel(state.docs, rel);
  state.docs = removeTab(state.docs, rel);
  if (state.active !== rel) {
    persistTabs();
    renderTabs();
    return;
  }
  if (next) {
    openDoc(next, state.docs.find((candidate) => candidate.rel === next)?.name);
  } else {
    state.active = null;
    persistTabs();
    renderTabs();
    showBrowser();
  }
}

// ── Panels and lightbox ─────────────────────────────────────────────────────

dom.tocToggle.addEventListener('click', () => {
  dom.toc.hidden = !dom.toc.hidden;
});

dom.notesToggle.addEventListener('click', () => {
  dom.notesPanel.hidden = !dom.notesPanel.hidden;
});

function openLightbox(src) {
  dom.lightboxImg.src = src;
  dom.lightbox.hidden = false;
}

function closeLightbox() {
  dom.lightbox.hidden = true;
  dom.lightboxImg.src = '';
}

dom.lightbox.addEventListener('click', closeLightbox);
dom.docMd.addEventListener('click', (event) => {
  const img = event.target.closest?.('img');
  if (img && img.src) openLightbox(img.src);
});

// ── Notes wiring ────────────────────────────────────────────────────────────

document.addEventListener('mouseup', () => {
  if (!dom.viewer.hidden) noteController.handleSelection();
});

document.addEventListener('mousedown', (event) => {
  if (!dom.toolbar.contains(event.target)) noteController.hideToolbar();
});

dom.annotate.addEventListener('mousedown', (event) => event.preventDefault());
dom.annotate.addEventListener('click', () => noteController.beginNote());
dom.noteSave.addEventListener('click', () => noteController.submit());
dom.noteCancel.addEventListener('click', () => noteController.closePopover());
dom.popoverClose.addEventListener('click', () => noteController.closePopover());
dom.noteText.addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    noteController.submit();
  }
});

document.addEventListener('keydown', (event) => {
  if (event.key !== 'Escape') return;
  if (!dom.lightbox.hidden) {
    closeLightbox();
    return;
  }
  if (!dom.popover.hidden) {
    noteController.closePopover();
    return;
  }
  noteController.hideToolbar();
  if (!dom.notesPanel.hidden) {
    dom.notesPanel.hidden = true;
    return;
  }
  if (!dom.toc.hidden) dom.toc.hidden = true;
});

// ── Boot ────────────────────────────────────────────────────────────────────

async function init() {
  updateTheme();
  dom.fontSlider.value = String(readSetting(FONT_KEY, FONT_DEFAULT, FONT_RANGE));
  dom.figureSlider.value = String(readSetting(FIGURE_KEY, FIGURE_DEFAULT, FIGURE_RANGE));
  applyTypography();

  try {
    const info = await api.getState();
    state.startRel = info.rel ?? '';
  } catch {
    state.startRel = '';
  }
  await loadLevel(state.startRel);

  const restored = loadTabs(localStorage);
  state.docs = restored.tabs;
  state.active = null;
  renderTabs();
  if (restored.active) {
    openDoc(restored.active, restored.tabs.find((tab) => tab.rel === restored.active)?.name);
  }
}

void init();
