/**
 * `docfoo kg --vis` page shell.
 *
 * Owns every DOM/network concern: loads state + graph, polls the query event
 * cursor, drives the canvas renderer through the pure reducer/choreography
 * modules, and manages the chat transcript, evidence strip, replay transport,
 * model picker, scope switch and theme.
 */

import { loadGraphData } from './graph.js';
import { computeLayout } from './layout.js';
import {
  parseStageFrame,
  computeStateAt,
  takeCapture,
  vizApplyFrame,
  vizCurrent,
  vizReset,
} from './state.js';
import {
  buildHighlights,
  cadenceFor,
  expansionBanner,
  retryBanner,
  routingBanner,
  stripForDelivered,
  stripForReorder,
} from './choreo.js';
import { VizRenderer } from './renderer.js';
import { renderMarkdown } from './markdown.js';
import {
  coerceModels,
  modelKey,
  modelLabel,
  nextReasoning,
  normalizeReasoning,
  providerLabel,
} from './models.js';
import { applyTheme, readTheme, toggleTheme, writeTheme } from './theme.js';

const byId = (id) => document.getElementById(id);

const dom = {
  body: document.body,
  kgBody: byId('kg-body'),
  scopeSelect: byId('scope-select'),
  modelButton: byId('model-button'),
  modelProvider: byId('model-provider'),
  modelName: byId('model-name'),
  reasoningButton: byId('reasoning-toggle'),
  reasoningLevel: byId('reasoning-level'),
  themeButton: byId('theme-toggle'),
  themeLabel: byId('theme-label'),
  modelPopover: byId('model-popover'),
  modelSearch: byId('model-search'),
  modelList: byId('model-list'),
  modelError: byId('model-error'),
  messages: byId('kgw-msgs'),
  chatEmpty: byId('kgw-empty'),
  chatScope: byId('kgw-scope'),
  composerModel: byId('composer-model'),
  input: byId('kgw-input'),
  askButton: byId('btn-kgw-ask'),
  stopButton: byId('btn-kgw-stop'),
  scene: byId('kg-viz-stage'),
  baseCanvas: byId('viz-base'),
  topCanvas: byId('viz-top'),
  pulse: byId('kg-viz-pulse'),
  status: byId('kg-viz-status'),
  meta: byId('kg-viz-meta'),
  banner: byId('kg-viz-banner'),
  bannerMessage: byId('kg-viz-banner-msg'),
  bannerChips: byId('kg-viz-chips'),
  noGraph: byId('kg-viz-nograph'),
  evidenceList: byId('kg-viz-evlist'),
  evidenceCount: byId('kg-viz-evcount'),
  evidenceWait: byId('kg-viz-evwait'),
  triples: byId('kg-viz-triples'),
  summary: byId('kg-viz-summary'),
  labelsInput: byId('kg-viz-labels'),
  fontSlider: byId('kg-font-slider'),
  figureSlider: byId('kg-figure-slider'),
  segChat: byId('seg-chat'),
  segGraph: byId('seg-graph'),
  transport: byId('kg-viz-transport'),
  replayPlayPause: byId('replay-playpause'),
  replaySeek: byId('replay-seek'),
  replayLabel: byId('replay-label'),
  replayExit: byId('replay-exit'),
  popover: byId('kg-viz-popover'),
  popName: byId('kg-viz-pop-name'),
  popType: byId('kg-viz-pop-type'),
  popMeta: byId('kg-viz-pop-meta'),
  popDesc: byId('kg-viz-pop-desc'),
  popSections: byId('kg-viz-pop-sections'),
  lightbox: byId('kg-lightbox'),
  lightboxImage: byId('kg-lightbox-image'),
  lightboxClose: byId('kg-lightbox-close'),
};

// ── Shell state ───────────────────────────────────────────────────────────

const shell = {
  workspace: '',
  scope: '',
  built: [],
  graphExists: false,
  model: null,
  reasoning: 'off',
};

const catalog = { providers: [] };
let sceneGraph = null;
let sceneLayout = null;
let sceneReady = false;
let sceneToken = 0;
const layoutCache = new Map();
let openNode = null;
let lastFrame = null;
let lastEndedKind = null;
let lastStripTitles = [];
let renderer = null;

const turn = {
  active: false,
  frames: [],
  cursor: 0,
  pollTimer: null,
  polling: false,
  bubble: null,
  deltaTimer: null,
};

const replay = { frames: [], index: 0, playing: false, timer: null };

// ── Small helpers ─────────────────────────────────────────────────────────

const setText = (element, value) => {
  if (element) element.textContent = String(value ?? '');
};

const scopeLabel = (scope) => (scope ? scope : 'Resources — top level');

const isNarrow = () =>
  typeof window.matchMedia === 'function'
  && window.matchMedia('(max-width: 719.98px)').matches;

function showView(view) {
  dom.kgBody.dataset.kgView = view === 'graph' ? 'graph' : 'chat';
  dom.segChat.setAttribute('aria-selected', String(view !== 'graph'));
  dom.segGraph.setAttribute('aria-selected', String(view === 'graph'));
}

function scrollMessages() {
  dom.messages.scrollTop = dom.messages.scrollHeight;
}

function autoGrowInput() {
  dom.input.style.height = 'auto';
  dom.input.style.height = `${Math.min(dom.input.scrollHeight, 120)}px`;
}

function setStreamingUi(active) {
  dom.askButton.disabled = active;
  dom.input.disabled = active;
  dom.stopButton.hidden = !active;
}

async function getJson(url) {
  const response = await fetch(url, { headers: { Accept: 'application/json' } });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(payload.error || `request failed (${response.status})`);
  return payload;
}

async function postJson(url, body) {
  const response = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify(body),
  });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(payload.error || `request failed (${response.status})`);
  return payload;
}

// ── Scene: graph → layout → renderer ──────────────────────────────────────

async function refreshScene() {
  const token = ++sceneToken;
  sceneReady = false;
  sceneGraph = null;
  sceneLayout = null;
  closePopover();
  renderer.clearScene();
  syncEmptyStates();

  const graph = await loadGraphData(shell.scope);
  if (token !== sceneToken) return;
  shell.graphExists = Boolean(graph);
  if (!graph) {
    syncEmptyStates();
    return;
  }

  let entry = layoutCache.get(graph.hash);
  if (!entry) {
    entry = { graph, layout: computeLayout(graph) };
    layoutCache.set(graph.hash, entry);
    while (layoutCache.size > 2) layoutCache.delete(layoutCache.keys().next().value);
  }
  sceneGraph = entry.graph;
  sceneLayout = entry.layout;
  sceneReady = true;
  renderer.setScene(sceneGraph, sceneLayout);
  syncEmptyStates();
  if (lastFrame) updateHighlights(vizCurrent());
}

function syncEmptyStates() {
  dom.noGraph.hidden = sceneReady;
  dom.chatEmpty.hidden = sceneReady || turn.active;
}

function resizeCanvases() {
  const rect = dom.scene.getBoundingClientRect();
  const width = Math.round(rect.width);
  const height = Math.round(rect.height);
  if (!width || !height) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  for (const canvas of [dom.baseCanvas, dom.topCanvas]) {
    const bufferWidth = Math.max(1, Math.round(width * dpr));
    const bufferHeight = Math.max(1, Math.round(height * dpr));
    if (canvas.width !== bufferWidth || canvas.height !== bufferHeight) {
      canvas.width = bufferWidth;
      canvas.height = bufferHeight;
    }
  }
  renderer.invalidate();
}

const canvasSpec = () => {
  const rect = dom.scene.getBoundingClientRect();
  return {
    cssW: Math.round(rect.width),
    cssH: Math.round(rect.height),
    dpr: Math.min(window.devicePixelRatio || 1, 2),
  };
};

// ── Node popover ──────────────────────────────────────────────────────────

function closePopover() {
  openNode = null;
  dom.popover.hidden = true;
}

function positionPopover() {
  if (openNode === null || !sceneLayout || dom.popover.hidden) return;
  const point = sceneLayout.points[openNode];
  if (!point) return closePopover();
  const view = renderer.getView();
  const x = point.x * view.k + view.x;
  const y = point.y * view.k + view.y;
  const width = dom.popover.offsetWidth || 250;
  const height = dom.popover.offsetHeight || 160;
  let left = x + 12;
  if (left + width > dom.scene.clientWidth - 8) left = Math.max(8, x - width - 12);
  const top = Math.min(Math.max(8, y - height / 2), Math.max(8, dom.scene.clientHeight - height - 8));
  dom.popover.style.left = `${Math.round(left)}px`;
  dom.popover.style.top = `${Math.round(top)}px`;
}

function openPopover(index) {
  const node = sceneGraph?.nodes[index];
  if (!node) return;
  const [id, name, type, degree] = node;
  openNode = index;

  setText(dom.popName, name || id);
  const hub = (sceneLayout?.hubs ?? []).includes(index);
  setText(dom.popType, `${type || 'NODE'}${hub ? ' · HUB' : ''}`);

  const links = [];
  for (const [title, section] of Object.entries(sceneGraph.sections || {})) {
    if (section.e.includes(index)) links.push({ title, topic: section.t });
  }
  links.sort((a, b) => a.title.localeCompare(b.title));
  setText(dom.popMeta, `degree ${degree} · ${links.length} linked section${links.length === 1 ? '' : 's'}`);
  const description = sceneGraph.descs?.[index] || '';
  setText(dom.popDesc, description || `No description recorded for “${name || id}”.`);

  dom.popSections.replaceChildren();
  if (!links.length) {
    const item = document.createElement('li');
    item.className = 'none';
    item.textContent = '— no linked sections in this graph —';
    dom.popSections.appendChild(item);
  } else {
    for (const link of links) {
      const item = document.createElement('li');
      const title = document.createElement('span');
      title.className = 't';
      title.textContent = link.title;
      item.appendChild(title);
      if (link.topic) {
        const topic = document.createElement('i');
        topic.textContent = link.topic;
        item.appendChild(topic);
      }
      dom.popSections.appendChild(item);
    }
  }
  dom.popover.hidden = false;
  positionPopover();
}

// ── Status bar, banner, evidence strip ────────────────────────────────────

const STATUS_BY_STEP = {
  depth: (data) => (data.depth === 'deep' ? 'Deep dive' : data.depth === 'simple' ? 'Simple lookup' : null),
  bm25: () => 'Scanning seeds…',
  gate: (data) => {
    if (!data.fired) {
      return typeof data.ratio === 'number'
        ? `Seeds strong (${Number(data.ratio).toFixed(1)}× floor)`
        : 'Seeds strong';
    }
    return data.conceptRouting === true
      ? 'Seeds weak - using Jev…'
      : 'Seeds weak - using LLM expansion…';
  },
  expansion: () => 'Bridging terms…',
  expansionFailed: () => 'Bridge unavailable — proceeding bare…',
  concepts: (data) => (Array.isArray(data.picks) && data.picks.length
    ? `Using Jev - ${data.picks.length} concepts matched…`
    : null),
  elevation: () => 'Elevating bridge leads…',
  seeds: (data) => (Array.isArray(data.ids) ? `${data.ids.length} seeds locked` : 'Seeds locked'),
  votes: () => 'Weighing guide topics…',
  descent: () => 'Descending guides…',
  hop: (data) => `Walking the graph — hop ${typeof data.hop === 'number' ? data.hop : '?'}…`,
  traversal: (data) => (typeof data.visitedCount === 'number'
    ? `Walk complete — ${data.visitedCount} reached`
    : 'Walk complete'),
  scored: () => 'Scoring sections…',
  delivered: () => 'Gathering sections…',
  reorder: (data) => (typeof data.droppedTotal === 'number' && data.droppedTotal > 0
    ? `Ordering evidence — ${data.droppedTotal} trimmed`
    : 'Ordering evidence…'),
  evidenceSummary: () => 'Evidence sealed',
  synthesis: (data) => (data.phase === 'start' ? 'Writing…' : null),
  retry: (data) => {
    const terms = Array.isArray(data.avoidTerms)
      ? data.avoidTerms.filter((term) => typeof term === 'string')
      : [];
    return terms.length
      ? `Pass refused — retrying without ${terms.slice(0, 3).join(', ')}`
      : 'Pass refused — retrying';
  },
};

function beatFor(frame) {
  const announce = STATUS_BY_STEP[frame.step];
  return announce ? announce(frame.data) : null;
}

function setStatus(text) {
  setText(dom.status, text);
}

function setMeta(frame) {
  let extra = '';
  if (frame.step === 'hop' && typeof frame.data.visitedSoFar === 'number') {
    extra = ` · ${frame.data.visitedSoFar} reached`;
  } else if (frame.step === 'traversal' && typeof frame.data.visitedCount === 'number') {
    extra = ` · ${frame.data.visitedCount} reached`;
  }
  setText(dom.meta, `pass ${frame.pass} · #${String(frame.seq).padStart(3, '0')}${extra}`);
  dom.meta.hidden = false;
}

function setBanner(spec) {
  if (!spec) {
    dom.banner.hidden = true;
    dom.banner.classList.remove('error');
    return;
  }
  dom.banner.hidden = false;
  dom.banner.classList.toggle('error', spec.variant === 'error');
  setText(dom.bannerMessage, spec.msg);
  dom.bannerChips.replaceChildren();
  for (const chip of spec.chips || []) {
    const element = document.createElement('span');
    element.className = 'kg-viz-chip';
    element.textContent = chip;
    dom.bannerChips.appendChild(element);
  }
}

function renderStrip(view, { flashRows, countText } = {}) {
  dom.evidenceList.classList.toggle('prov', Boolean(view.provisional));
  const fragment = document.createDocumentFragment();
  view.rows.forEach((row, index) => {
    const item = document.createElement('li');
    if (row.dropped) item.className = 'dropped';
    else if (flashRows?.has(index)) item.className = 'flash';
    const number = document.createElement('i');
    number.textContent = row.direct ? '⌖' : String(index + 1);
    if (row.direct) number.className = 'direct';
    const title = document.createElement('span');
    title.className = 't';
    title.textContent = row.title;
    item.append(number, title);
    if (row.chars !== undefined) {
      const chars = document.createElement('b');
      chars.textContent = `${Number(row.chars || 0).toLocaleString()} ch`;
      item.appendChild(chars);
    }
    fragment.appendChild(item);
  });
  if (view.hiddenMore > 0) {
    const more = document.createElement('li');
    more.className = 'more';
    more.textContent = `+${view.hiddenMore} more`;
    fragment.appendChild(more);
  }
  dom.evidenceList.replaceChildren(fragment);
  dom.evidenceWait.hidden = view.rows.length > 0;
  dom.evidenceCount.textContent = countText ?? String(view.total);
}

function updateHighlights(state) {
  if (sceneReady && sceneGraph && sceneLayout) {
    renderer.setHighlights(buildHighlights(state, sceneGraph, sceneLayout));
  }
}

/** Banner / strip / canvas side effects for one frame (live and replay). */
function choreograph(frame, state, restore = false) {
  switch (frame.step) {
    case 'expansion':
      setBanner(expansionBanner(state));
      break;
    case 'expansionFailed':
      setBanner({ msg: state.expansionError ?? 'Term bridge unavailable', chips: [], variant: 'error' });
      break;
    case 'concepts':
      setBanner(routingBanner(state));
      break;
    case 'retry':
      renderer.dropHighlights();
      dom.evidenceList.replaceChildren();
      dom.evidenceWait.hidden = false;
      setText(dom.evidenceWait, 're-retrieving — pass 2');
      dom.evidenceCount.textContent = '—';
      lastStripTitles = [];
      setBanner(retryBanner(state));
      break;
    case 'delivered': {
      const total = state.tierTotal ?? state.tierOrder.length;
      renderStrip(stripForDelivered(state.tierOrder, total, state.directHits), { countText: String(total) });
      break;
    }
    case 'reorder': {
      const result = stripForReorder(
        lastStripTitles,
        state.finalOrder,
        state.finalTotal ?? state.finalOrder.length,
        state.droppedSections,
        state.droppedTotal,
        state.directHits,
      );
      lastStripTitles = state.finalOrder.map((row) => row.section);
      renderStrip(result.view, {
        flashRows: restore ? undefined : result.movedRows,
        countText: state.droppedTotal > 0 ? `${state.finalTotal} · −${state.droppedTotal}` : String(state.finalTotal),
      });
      setBanner(null);
      break;
    }
    case 'evidenceSummary':
      setText(dom.triples, state.triples == null ? '' : `triples ${state.triples}`);
      break;
    default:
      break;
  }
  updateHighlights(state);
}

/** Shared beat painter for live frames and replay ticks. */
function applyBeat(frame, state, restore = false) {
  const beat = beatFor(frame);
  if (beat !== null) setStatus(beat);
  choreograph(frame, state, restore);
  return beat;
}

function noteStage(frame) {
  lastFrame = frame;
  const beat = applyBeat(frame, vizCurrent());
  setMeta(frame);
  return beat;
}

// ── Query lifecycle ───────────────────────────────────────────────────────

function clearPollTimer() {
  if (turn.pollTimer !== null) clearTimeout(turn.pollTimer);
  turn.pollTimer = null;
}

function markQueryStart() {
  if (replay.frames.length) stopReplay();
  vizReset();
  lastFrame = null;
  lastEndedKind = null;
  lastStripTitles = [];
  closePopover();
  dom.pulse.classList.add('live');
  dom.pulse.classList.remove('replay');
  dom.status.classList.remove('replay');
  setStatus('Idle');
  dom.meta.hidden = true;
  renderer.dropHighlights();
  setBanner(null);
  dom.evidenceList.replaceChildren();
  dom.evidenceWait.hidden = false;
  setText(dom.evidenceWait, 'awaits the first answer');
  dom.evidenceCount.textContent = '—';
  dom.triples.textContent = '';
  dom.summary.textContent = '';
  if (isNarrow()) showView('graph');
}

function markQueryEnd(kind, message) {
  dom.pulse.classList.remove('live');
  lastEndedKind = kind === 'error' ? null : kind;

  if (kind === 'error') {
    renderer.dropHighlights();
    setStatus('Failed — see chat');
    setBanner({
      msg: message ? `Retrieval failed — ${message}` : 'Retrieval failed',
      chips: [],
      variant: 'error',
    });
    return;
  }
  renderer.setPatina(true);
  setStatus(kind === 'cancelled' ? 'Stopped.' : 'Idle');
  setBanner(null);
}

function addSystem(text) {
  const element = document.createElement('div');
  element.className = 'chat-message system';
  element.textContent = text;
  dom.messages.appendChild(element);
  scrollMessages();
}

function addUser(text) {
  const element = document.createElement('div');
  element.className = 'chat-message user';
  element.textContent = text;
  dom.messages.appendChild(element);
  scrollMessages();
}

function createAssistant() {
  const bubble = document.createElement('div');
  bubble.className = 'chat-message assistant streaming';
  const typing = document.createElement('div');
  typing.className = 'chat-typing-text';
  typing.textContent = 'Working…';
  const body = document.createElement('div');
  body.className = 'markdown-content';
  bubble.append(typing, body);
  dom.messages.appendChild(bubble);
  scrollMessages();
  return { element: bubble, typing, body, raw: '' };
}

function paintBubble(bubble) {
  bubble.body.innerHTML = renderMarkdown(bubble.raw);
  stampFigureWidths(bubble.body);
}

function finishWithMarkdown(bubble, markdown) {
  bubble.raw = markdown;
  bubble.typing.remove();
  bubble.element.classList.remove('streaming');
  paintBubble(bubble);
}

async function startQuery(rawText) {
  const question = String(rawText || '').trim();
  if (!question || turn.active) return;

  if (!shell.model) {
    addSystem('No model selected — choose one from the model menu first.');
    return;
  }

  closeModelPicker();
  turn.active = true;
  turn.frames = [];
  turn.cursor = 0;
  turn.bubble = null;
  turn.deltaTimer = null;

  markQueryStart();
  dom.chatEmpty.hidden = true;
  addUser(question);
  turn.bubble = createAssistant();
  setStreamingUi(true);

  // Start polling only once the server has begun the turn. `begin_query`
  // clears the event log before answering 202, so a poll from cursor 0 can
  // never replay the previous turn's frames into this answer.
  try {
    await postJson('/api/query', {
      query: question,
      scope: shell.scope,
      model: shell.model,
      reasoning: shell.reasoning,
    });
  } catch (error) {
    if (!turn.active) return;
    finishError(error instanceof Error ? error.message : String(error));
    return;
  }
  if (turn.active) startPolling();
}

async function stopQuery() {
  if (!turn.active) return;
  // Freeze the canvas immediately so the affordance is honest before the
  // cancelled frame arrives.
  renderer.setPatina(true);
  setStatus('Stopped.');
  try {
    await postJson('/api/cancel', {});
  } catch {
    // The worker still reports its terminal frame; nothing else to do here.
  }
}

function startPolling() {
  clearPollTimer();
  void pollEvents();
}

async function pollEvents() {
  if (!turn.active || turn.polling) return;
  turn.polling = true;
  try {
    const payload = await getJson(`/api/events?since=${turn.cursor}`);
    if (typeof payload.cursor === 'number') {
      turn.cursor = Math.max(turn.cursor, payload.cursor);
    }
    for (const event of Array.isArray(payload.events) ? payload.events : []) {
      if (typeof event.id === 'number') turn.cursor = Math.max(turn.cursor, event.id);
      handleEvent(event);
      if (!turn.active) break;
    }
  } catch {
    // Transient loopback failures are retried by the next poll.
  } finally {
    turn.polling = false;
    if (turn.active) turn.pollTimer = window.setTimeout(pollEvents, 120);
  }
}

function handleEvent(event) {
  const data = event && typeof event.data === 'object' ? event.data : null;
  if (!data || !turn.active) return;

  switch (data.type) {
    case 'start':
      onStartFrame(data);
      break;
    case 'stage':
      onStageFrame(data);
      break;
    case 'delta':
      onDeltaFrame(data);
      break;
    case 'done':
      onDoneFrame(data);
      break;
    case 'cancelled':
      onCancelledFrame();
      break;
    case 'error':
      finishError(data.message || 'retrieval failed');
      break;
    default:
      break;
  }
}

/** Page opened mid-query: the start frame reconstructs the transcript. */
function onStartFrame(data) {
  if (!turn.bubble && typeof data.query === 'string' && data.query) {
    addUser(data.query);
    turn.bubble = createAssistant();
  }
  if (typeof data.model === 'string' && data.model) {
    shell.model = data.model;
    updateModelUi();
  }
}

function onStageFrame(data) {
  const frame = parseStageFrame(data);
  if (!frame) return;
  vizApplyFrame(frame);
  if (turn.frames.length < 600) turn.frames.push(frame);
  const beat = noteStage(frame);
  if (beat && turn.bubble && turn.bubble.typing.isConnected) {
    setText(turn.bubble.typing, beat);
    scrollMessages();
  }
}

function onDeltaFrame(data) {
  if (!turn.bubble) turn.bubble = createAssistant();
  const text = typeof data.text === 'string' ? data.text : '';
  if (turn.bubble.raw.length === 0 && text.length > 0) setStatus('Writing…');
  turn.bubble.raw += text;
  turn.bubble.typing.remove();
  if (turn.deltaTimer === null) {
    turn.deltaTimer = window.setTimeout(() => {
      turn.deltaTimer = null;
      if (turn.bubble) paintBubble(turn.bubble);
    }, 80);
  }
  scrollMessages();
}

function onDoneFrame(done) {
  if (turn.deltaTimer !== null) {
    clearTimeout(turn.deltaTimer);
    turn.deltaTimer = null;
  }
  turn.active = false;
  clearPollTimer();
  setStreamingUi(false);

  const captured = takeCapture();
  const frames = turn.frames.length ? turn.frames : captured;
  const bubble = turn.bubble;
  turn.bubble = null;

  const markdown = typeof done.answer_markdown === 'string' && done.answer_markdown
    ? done.answer_markdown
    : (typeof done.answer === 'string' ? done.answer : '');
  if (bubble) {
    finishWithMarkdown(bubble, markdown);
    if (hasSources(done, frames)) bubble.element.appendChild(buildSourcesRow(done, frames));
  } else {
    const restored = createAssistant();
    finishWithMarkdown(restored, markdown);
    if (hasSources(done, frames)) restored.element.appendChild(buildSourcesRow(done, frames));
  }

  renderTimings(done.timings, done.totalSecs);
  markQueryEnd('done');
  dom.chatEmpty.hidden = true;
  scrollMessages();
}

function onCancelledFrame() {
  turn.active = false;
  clearPollTimer();
  setStreamingUi(false);
  takeCapture();
  const bubble = turn.bubble;
  turn.bubble = null;
  if (bubble) {
    bubble.typing.remove();
    if (bubble.raw.trim()) {
      finishWithMarkdown(bubble, bubble.raw);
      const note = document.createElement('div');
      note.className = 'chat-note';
      note.textContent = 'Stopped.';
      bubble.element.appendChild(note);
    } else {
      bubble.element.remove();
    }
  }
  markQueryEnd('cancelled');
  scrollMessages();
}

function finishError(message) {
  if (!turn.active) return;
  turn.active = false;
  clearPollTimer();
  setStreamingUi(false);
  takeCapture();
  const bubble = turn.bubble;
  turn.bubble = null;
  if (bubble) {
    finishWithMarkdown(bubble, `**Error:** ${message}`);
  } else {
    const error = createAssistant();
    finishWithMarkdown(error, `**Error:** ${message}`);
  }
  markQueryEnd('error', message);
  scrollMessages();
}

// ── Sources row + timings ─────────────────────────────────────────────────

function sourceText(source) {
  if (typeof source === 'string') return source;
  if (!source || typeof source !== 'object') return '';
  const parts = [];
  if (source.section) parts.push(source.section);
  if (source.chars != null) parts.push(`${Number(source.chars).toLocaleString()} chars`);
  if (source.doc && source.start_line != null) {
    const lines = source.end_line != null && source.end_line > source.start_line
      ? `${source.start_line}-${source.end_line}`
      : String(source.start_line);
    parts.push(`${source.doc}:${lines}`);
  }
  return parts.join(' · ');
}

function routingBadge(routing) {
  if (!routing || typeof routing !== 'object') return null;
  const picks = Array.isArray(routing.picks) ? routing.picks : [];
  const terms = Array.isArray(routing.terms) ? routing.terms : [];
  if (routing.used && picks.length) {
    return {
      text: 'concept routed',
      title: picks.map((pick) => `${pick.name || pick.section} ${Math.round(Number(pick.prob || 0) * 100)}%`).join(', '),
    };
  }
  if (terms.length) return { text: routing.fallback ? 'escalated' : 'bridge terms', title: terms.join(', ') };
  if (routing.fallback) return { text: 'routing failed', title: String(routing.fallback) };
  if (routing.used) return { text: 'no concept match', title: 'Jev routing found no concept above the threshold.' };
  return null;
}

function hasSources(done, frames) {
  return Boolean((Array.isArray(done.sources) && done.sources.length) || frames.length || routingBadge(done.routing));
}

function buildSourcesRow(done, frames) {
  const sources = Array.isArray(done.sources) ? done.sources : [];
  const details = document.createElement('details');
  details.className = 'kg-sources';

  const summary = document.createElement('summary');
  summary.appendChild(document.createTextNode(`Sources · ${sources.length} section${sources.length === 1 ? '' : 's'}`));
  const seconds = Number(done.totalSecs);
  if (Number.isFinite(seconds) && seconds > 0) {
    summary.appendChild(document.createTextNode(` · ${seconds.toFixed(1)}s`));
  }
  const badge = routingBadge(done.routing);
  if (badge) {
    const element = document.createElement('span');
    element.className = 'kg-badge';
    element.textContent = badge.text;
    element.title = badge.title;
    summary.appendChild(element);
  }
  if (frames.length) {
    const replayButton = document.createElement('button');
    replayButton.type = 'button';
    replayButton.className = 'kg-replay-btn';
    replayButton.textContent = '▶';
    replayButton.title = `Replay this turn (${frames.length} beats)`;
    replayButton.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (!turn.active) playTrace(frames);
    });
    summary.appendChild(replayButton);
  }
  details.appendChild(summary);

  const list = document.createElement('ul');
  for (const source of sources) {
    const item = document.createElement('li');
    item.textContent = sourceText(source);
    list.appendChild(item);
  }
  details.appendChild(list);
  return details;
}

const TIMING_LABELS = {
  bm25: 'BM25', expansion: 'bridge', routing: 'routing', ancestor_voting: 'voting',
  guided_descent: 'descent', traversal: 'traverse', evidence: 'evidence', synthesis: 'writing',
  depth: 'classify', gate: 'gate', elevation: 'elevate', seeds: 'seeds', votes: 'vote',
  descent: 'descent', hop: 'traverse', scored: 'score', delivered: 'deliver', reorder: 'order',
  evidenceSummary: 'evidence', retry: 'retry',
};

function renderTimings(raw, totalSeconds) {
  const timings = Array.isArray(raw)
    ? raw.flatMap((entry) => {
        if (!entry || typeof entry.step !== 'string') return [];
        const secs = Number(entry.secs);
        return Number.isFinite(secs) && secs >= 0.005 ? [{ step: entry.step, secs }] : [];
      }).slice(0, 8)
    : [];
  if (!timings.length) {
    setText(dom.summary, Number(totalSeconds) > 0 ? `total ${Number(totalSeconds).toFixed(1)}s` : '');
    return;
  }
  setText(dom.summary, timings
    .map(({ step, secs }) => `${TIMING_LABELS[step] ?? step.toLowerCase()} ${secs >= 10 ? secs.toFixed(1) : secs.toFixed(2)}s`)
    .join(' · '));
}

// ── Replay ────────────────────────────────────────────────────────────────

function syncTransport() {
  dom.replaySeek.max = String(replay.frames.length);
  dom.replaySeek.value = String(replay.index);
  setText(dom.replayLabel, `${replay.index}/${replay.frames.length}`);
  dom.replayPlayPause.textContent = replay.playing
    ? '❚❚'
    : replay.index >= replay.frames.length ? '↻' : '▶';
}

function stopReplayTimer() {
  if (replay.timer !== null) clearTimeout(replay.timer);
  replay.timer = null;
  replay.playing = false;
}

function paintReplayFrame() {
  const frame = replay.frames[replay.index - 1];
  dom.pulse.classList.add('replay');
  dom.status.classList.add('replay');
  dom.banner.classList.add('noanim');
  try {
    if (frame) {
      applyBeat(frame, computeStateAt(replay.frames, replay.index), true);
      setText(dom.meta, `REPLAY ${replay.index}/${replay.frames.length}`);
    } else {
      renderer.dropHighlights();
      setStatus('Replay start');
      setText(dom.meta, `REPLAY 0/${replay.frames.length}`);
    }
    dom.meta.hidden = false;
  } finally {
    dom.banner.classList.remove('noanim');
  }
  syncTransport();
  positionPopover();
}

function scheduleReplayTick() {
  if (replay.timer !== null) clearTimeout(replay.timer);
  replay.timer = window.setTimeout(() => {
    replay.timer = null;
    if (!replay.playing) return;
    if (replay.index >= replay.frames.length) {
      replay.playing = false;
      syncTransport();
      return;
    }
    replay.index += 1;
    paintReplayFrame();
    scheduleReplayTick();
  }, cadenceFor(replay.frames.length));
}

function playTrace(rawFrames) {
  if (turn.active) return false;
  const frames = (rawFrames || []).map(parseStageFrame).filter(Boolean);
  if (!frames.length) return false;
  stopReplay();
  replay.frames = frames;
  replay.index = 1;
  replay.playing = true;
  dom.transport.hidden = false;
  closePopover();
  renderer.dropHighlights();
  if (isNarrow()) showView('graph');
  paintReplayFrame();
  scheduleReplayTick();
  return true;
}

function toggleReplay() {
  if (!replay.frames.length) return;
  if (!replay.playing && replay.index >= replay.frames.length) {
    replay.index = 1;
    replay.playing = true;
    paintReplayFrame();
    scheduleReplayTick();
    return;
  }
  if (replay.playing) {
    stopReplayTimer();
    syncTransport();
  } else {
    replay.playing = true;
    scheduleReplayTick();
    syncTransport();
  }
}

function seekReplay(value) {
  if (!replay.frames.length) return;
  stopReplayTimer();
  replay.index = Math.min(replay.frames.length, Math.max(0, Math.round(Number(value) || 0)));
  paintReplayFrame();
}

/** Live wins: exit replay and restore the live painting underneath. */
function stopReplay() {
  stopReplayTimer();
  const wasOpen = replay.frames.length > 0;
  replay.frames = [];
  replay.index = 0;
  dom.transport.hidden = true;
  dom.pulse.classList.remove('replay');
  dom.status.classList.remove('replay');
  closePopover();
  if (!wasOpen) return;
  if (lastFrame) {
    applyBeat(lastFrame, vizCurrent(), true);
    renderer.setPatina(Boolean(lastEndedKind));
  } else {
    renderer.dropHighlights();
  }
}

const isReplayPlaying = () => replay.playing;
const isReplayOpen = () => replay.frames.length > 0;

// ── Model picker + reasoning ──────────────────────────────────────────────

function updateModelUi() {
  setText(dom.modelProvider, providerLabel(shell.model, catalog.providers));
  setText(dom.modelName, shell.model ? modelLabel(shell.model, catalog.providers) : 'no model');
  setText(dom.composerModel, shell.model ? modelLabel(shell.model, catalog.providers) : 'no model selected');
}

function updateReasoningUi() {
  shell.reasoning = normalizeReasoning(shell.reasoning);
  setText(dom.reasoningLevel, shell.reasoning);
  dom.reasoningButton.setAttribute('aria-label', `Thinking level: ${shell.reasoning}`);
  dom.reasoningButton.classList.toggle('active', shell.reasoning !== 'off');
}

function renderModelList() {
  const filter = dom.modelSearch.value.trim().toLowerCase();
  dom.modelList.replaceChildren();
  let visible = 0;

  for (const provider of catalog.providers) {
    const models = provider.models.filter((model) => {
      if (!filter) return true;
      return modelKey(provider, model).toLowerCase().includes(filter)
        || (model.name || '').toLowerCase().includes(filter)
        || provider.name.toLowerCase().includes(filter);
    });
    if (!models.length && filter) continue;

    const group = document.createElement('section');
    group.className = 'model-group';
    const heading = document.createElement('div');
    heading.className = 'model-group-title';
    const name = document.createElement('span');
    name.textContent = provider.label || provider.name;
    const state = document.createElement('span');
    state.className = provider.configured ? 'model-state ok' : 'model-state';
    state.textContent = provider.configured ? 'configured' : 'not configured';
    heading.append(name, state);
    group.appendChild(heading);

    for (const model of models) {
      const key = modelKey(provider, model);
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'model-item';
      button.disabled = !provider.configured;
      button.classList.toggle('active', key === shell.model);
      const modelName = document.createElement('span');
      modelName.className = 'model-item-name';
      modelName.textContent = model.name || model.id;
      const modelId = document.createElement('span');
      modelId.className = 'model-item-id';
      modelId.textContent = model.id;
      const check = document.createElement('span');
      check.className = 'model-item-check';
      check.textContent = key === shell.model ? '✓' : '';
      button.append(modelName, modelId, check);
      button.addEventListener('click', () => {
        shell.model = key;
        updateModelUi();
        closeModelPicker();
      });
      group.appendChild(button);
      visible += 1;
    }
    dom.modelList.appendChild(group);
  }

  if (!visible) {
    const empty = document.createElement('p');
    empty.className = 'model-empty';
    empty.textContent = catalog.providers.length
      ? 'No models match this filter.'
      : 'Model catalog unavailable.';
    dom.modelList.appendChild(empty);
  }
}

let modelsPromise = null;

async function loadModels(refresh = false) {
  if (modelsPromise && !refresh) return modelsPromise;
  const pending = (async () => {
    setText(dom.modelError, '');
    dom.modelError.hidden = true;
    try {
      const payload = await getJson(`/api/models?refresh=${refresh ? 1 : 0}`);
      catalog.providers = coerceModels(payload);
      updateModelUi();
      renderModelList();
    } catch (error) {
      dom.modelError.hidden = false;
      setText(dom.modelError, error instanceof Error ? error.message : String(error));
      renderModelList();
    } finally {
      if (modelsPromise === pending) modelsPromise = null;
    }
  })();
  modelsPromise = pending;
  return pending;
}

function closeModelPicker() {
  dom.modelPopover.hidden = true;
  dom.modelButton.setAttribute('aria-expanded', 'false');
}

function openModelPicker() {
  dom.modelPopover.hidden = false;
  dom.modelButton.setAttribute('aria-expanded', 'true');
  renderModelList();
  dom.modelSearch.focus();
  if (!catalog.providers.length) void loadModels(false);
}

// ── Scope + header ────────────────────────────────────────────────────────

function populateScopes() {
  const scopes = [...new Set([...shell.built, shell.scope])].sort((a, b) => {
    if (a === '') return -1;
    if (b === '') return 1;
    return a.localeCompare(b);
  });
  dom.scopeSelect.replaceChildren();
  for (const scope of scopes) {
    const option = document.createElement('option');
    option.value = scope;
    option.textContent = scopeLabel(scope);
    dom.scopeSelect.appendChild(option);
  }
  if (!scopes.length) {
    const option = document.createElement('option');
    option.value = shell.scope;
    option.textContent = scopeLabel(shell.scope);
    dom.scopeSelect.appendChild(option);
  }
  dom.scopeSelect.value = shell.scope;
  setText(dom.chatScope, scopeLabel(shell.scope));
}

async function changeScope() {
  if (turn.active || isReplayOpen()) return;
  shell.scope = dom.scopeSelect.value;
  setText(dom.chatScope, scopeLabel(shell.scope));
  dom.chatEmpty.hidden = true;
  await refreshScene();
}

function updateThemeUi() {
  currentTheme = applyTheme(currentTheme, dom.body);
  renderer?.invalidate();
  const current = currentTheme === 'kinetic' ? 'Kinetic' : 'Art Deco';
  const next = currentTheme === 'kinetic' ? 'Art Deco' : 'Kinetic';
  setText(dom.themeLabel, current);
  dom.themeButton.title = `Switch to ${next}`;
  dom.themeButton.setAttribute('aria-label', dom.themeButton.title);
}

let currentTheme = 'art-deco';
const shellTheme = () => currentTheme;

// ── Lightbox ──────────────────────────────────────────────────────────────

function openLightbox(image) {
  dom.lightboxImage.src = image.currentSrc || image.src;
  dom.lightboxImage.alt = image.alt || 'Figure';
  dom.lightbox.hidden = false;
}

function closeLightbox() {
  dom.lightbox.hidden = true;
  dom.lightboxImage.removeAttribute('src');
}

// ── Boot ──────────────────────────────────────────────────────────────────

function resetTranscript() {
  dom.messages.replaceChildren();
  addSystem(
    'Ask about your indexed scans — answers are retrieved from the knowledge graph and the source sections are listed under each answer. Questions are independent, so include the context each question needs.',
  );
}

async function loadState() {
  try {
    const state = await getJson('/api/state');
    shell.workspace = typeof state.workspace === 'string' ? state.workspace : '';
    shell.scope = typeof state.scope === 'string' ? state.scope : '';
    shell.built = Array.isArray(state.built) ? state.built.filter((scope) => typeof scope === 'string') : [];
    shell.graphExists = state.graphExists === true;
    shell.model = typeof state.model === 'string' && state.model ? state.model : null;
    shell.reasoning = normalizeReasoning(state.reasoning);
    if (state.busy === true) resumeRunningQuery();
  } catch (error) {
    addSystem(error instanceof Error ? error.message : String(error));
  }

  populateScopes();
  updateModelUi();
  updateReasoningUi();
  await refreshScene();
}

/** The server was already running a query when this page loaded. */
function resumeRunningQuery() {
  turn.active = true;
  turn.frames = [];
  turn.cursor = 0;
  turn.bubble = null;
  turn.deltaTimer = null;
  markQueryStart();
  dom.chatEmpty.hidden = true;
  setStreamingUi(true);
  startPolling();
}

// ── Answer text + figure size sliders (desktop parity) ────────────────────

// Ranges and the font default match `initKgFont`/`initKgFigure`; the figure
// default is the middle of its range (the desktop starts it at max).
const FONT_SLIDER = { key: 'docfoo-kg-vis-font', min: 12, max: 45, fallback: 20 };
const FIGURE_SLIDER = { key: 'docfoo-kg-vis-figure', min: 25, max: 100, fallback: 65 };

function storedSliderValue(config, fallback) {
  try {
    const raw = window.localStorage.getItem(config.key);
    const value = Number(raw);
    if (raw !== null && raw !== '' && Number.isFinite(value)) {
      return Math.min(config.max, Math.max(config.min, value));
    }
  } catch {
    // Storage disabled: use the default.
  }
  return fallback;
}

function saveSliderValue(config, value) {
  try {
    window.localStorage.setItem(config.key, String(value));
  } catch {
    // Session-only when storage is unavailable.
  }
}

function initSliders() {
  const font = storedSliderValue(FONT_SLIDER, FONT_SLIDER.fallback);
  dom.fontSlider.value = String(font);
  document.documentElement.style.setProperty('--kg-font-size', `${font}px`);
  dom.fontSlider.addEventListener('input', () => {
    const value = Number(dom.fontSlider.value);
    if (!Number.isFinite(value)) return;
    document.documentElement.style.setProperty('--kg-font-size', `${value}px`);
    saveSliderValue(FONT_SLIDER, value);
  });

  const figure = storedSliderValue(FIGURE_SLIDER, FIGURE_SLIDER.fallback);
  dom.figureSlider.value = String(figure);
  document.documentElement.style.setProperty('--kg-figure-scale', String(figure / 100));
  dom.figureSlider.addEventListener('input', () => {
    const value = Number(dom.figureSlider.value);
    if (!Number.isFinite(value)) return;
    document.documentElement.style.setProperty('--kg-figure-scale', String(value / 100));
    saveSliderValue(FIGURE_SLIDER, value);
  });
}

/** Stamp natural widths so the figure slider can scale images down. */
function stampFigureWidths(root) {
  for (const image of root.querySelectorAll('img.kg-figure')) {
    const stamp = () => {
      if (image.naturalWidth > 0) image.style.setProperty('--img-w', `${image.naturalWidth}px`);
    };
    if (image.complete) stamp();
    else image.addEventListener('load', stamp, { once: true });
  }
}

function initRenderer() {
  renderer = new VizRenderer({ getSpec: canvasSpec, labelsOn: () => dom.labelsInput.checked });
  renderer.baseContext = dom.baseCanvas.getContext('2d');
  renderer.topContext = dom.topCanvas.getContext('2d');
  renderer.attach(dom.scene, (dragging) => dom.scene.classList.toggle('panning', dragging));
  renderer.setPickHandler((index) => {
    if (index === null || index === openNode) closePopover();
    else openPopover(index);
  });
  renderer.onView(positionPopover);
  dom.popover.querySelector('#kg-viz-pop-close').addEventListener('click', closePopover);

  if (typeof ResizeObserver !== 'undefined') {
    new ResizeObserver(resizeCanvases).observe(dom.scene);
  }
  window.addEventListener('resize', resizeCanvases, { passive: true });
  resizeCanvases();
}

function initControls() {
  currentTheme = applyTheme(readTheme(window.localStorage), dom.body);
  updateThemeUi();
  initSliders();

  try {
    dom.labelsInput.checked = window.localStorage.getItem('docfoo-kg-vis-labels') !== '0';
  } catch {
    dom.labelsInput.checked = true;
  }
  dom.labelsInput.addEventListener('change', () => {
    try {
      window.localStorage.setItem('docfoo-kg-vis-labels', dom.labelsInput.checked ? '1' : '0');
    } catch {
      // Storage disabled; the session still applies the toggle.
    }
    renderer.invalidate();
  });

  dom.themeButton.addEventListener('click', () => {
    currentTheme = writeTheme(window.localStorage, toggleTheme(currentTheme));
    updateThemeUi();
  });

  dom.reasoningButton.addEventListener('click', () => {
    shell.reasoning = nextReasoning(shell.reasoning);
    updateReasoningUi();
  });

  dom.modelButton.addEventListener('click', (event) => {
    event.stopPropagation();
    if (dom.modelPopover.hidden) openModelPicker();
    else closeModelPicker();
  });
  dom.modelPopover.addEventListener('click', (event) => event.stopPropagation());
  document.addEventListener('click', closeModelPicker);
  dom.modelSearch.addEventListener('input', renderModelList);
  byId('models-refresh').addEventListener('click', () => void loadModels(true));

  dom.scopeSelect.addEventListener('change', () => void changeScope());
  dom.segChat.addEventListener('click', () => showView('chat'));
  dom.segGraph.addEventListener('click', () => showView('graph'));

  dom.askButton.addEventListener('click', () => {
    const value = dom.input.value.trim();
    if (!value) return;
    dom.input.value = '';
    autoGrowInput();
    void startQuery(value);
  });
  dom.stopButton.addEventListener('click', () => void stopQuery());
  dom.input.addEventListener('input', autoGrowInput);
  dom.input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      dom.askButton.click();
    }
  });

  dom.replayPlayPause.addEventListener('click', toggleReplay);
  dom.replayExit.addEventListener('click', stopReplay);
  dom.replaySeek.addEventListener('input', () => seekReplay(dom.replaySeek.value));

  dom.messages.addEventListener('click', (event) => {
    const target = event.target instanceof Element ? event.target.closest('img.kg-figure') : null;
    if (target) openLightbox(target);
  });
  dom.lightbox.addEventListener('click', (event) => {
    if (event.target === dom.lightbox) closeLightbox();
  });
  dom.lightboxClose.addEventListener('click', closeLightbox);

  document.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    if (!dom.lightbox.hidden) closeLightbox();
    else if (!dom.popover.hidden) closePopover();
    else if (!dom.modelPopover.hidden) closeModelPicker();
  });
}

async function init() {
  resetTranscript();
  initRenderer();
  initControls();
  showView('chat');
  await loadState();
}

void init();

export {
  startQuery,
  stopQuery,
  playTrace,
  toggleReplay,
  seekReplay,
  stopReplay,
  isReplayPlaying,
  isReplayOpen,
};
