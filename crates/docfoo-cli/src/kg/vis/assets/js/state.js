/**
 * KG stage-frame wire contract + pure reducer.
 *
 * `applyStageFrame` is a pure transition (returns a new state, never mutates
 * its input). The live query and the replay player run the exact same code
 * path; replay just folds recorded frames with `computeStateAt`.
 *
 * Every consumer must ignore unknown steps/fields (forward compatibility).
 * Lists are already capped at 64 entries by the Rust stage events.
 */

/** Tolerant wire parser: rejects non-stage payloads, never throws. */
export function parseStageFrame(payload) {
  if (typeof payload !== 'object' || payload === null) return null;
  if (payload.type !== 'stage') return null;
  if (typeof payload.step !== 'string' || payload.step.length === 0) return null;
  const data =
    typeof payload.data === 'object' && payload.data !== null && !Array.isArray(payload.data)
      ? payload.data
      : {};
  const pass = typeof payload.pass === 'number' ? payload.pass : 1;
  const seq = typeof payload.seq === 'number' ? payload.seq : 0;
  return { type: 'stage', pass, seq, step: payload.step, data };
}

/** Fresh state before a turn. */
export function createVizState() {
  return {
    pass: 1,
    seq: 0,
    lastStep: null,

    // Question classification
    depth: null,

    // bm25 / gate
    poolSize: null,
    sectionHits: null,
    gateFired: null,
    gateRatio: null,

    // legacy expansion path
    escalationTerms: [],
    expansionError: null,

    // Jev concept routing
    routingUsed: null,
    routingModel: null,
    routingCandidates: null,
    routePicks: [],
    routingError: null,
    routingSecs: null,

    // seeds
    seeds: [],
    anchors: [],
    leads: [],

    // guides + descent
    guides: [],
    votes: [],
    picks: [],

    // traversal
    hops: [],
    visited: new Set(),
    visitedCount: null,

    // sections
    directHits: [],
    tierOrder: [],
    tierTotal: null,
    finalOrder: [],
    finalTotal: null,
    sections: [],
    droppedSections: [],
    droppedTotal: 0,
    reordered: false,
    triples: null,

    // synthesis + ending
    writing: false,
    writtenChars: null,
    retryActive: false,
    avoidTerms: [],
    finished: false,
  };
}

const asStringArray = (value) =>
  Array.isArray(value) ? value.filter((item) => typeof item === 'string') : [];

const asNumber = (value) => {
  const number = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(number) ? number : null;
};

function asSectionRows(value) {
  if (!Array.isArray(value)) return [];
  return value
    .flatMap((item) => {
      if (typeof item !== 'object' || item === null) return [];
      if (typeof item.section !== 'string') return [];
      return [{ section: item.section, chars: asNumber(item.chars) ?? 0 }];
    })
    .slice(0, 64);
}

function asConceptPicks(value) {
  if (!Array.isArray(value)) return [];
  return value
    .flatMap((item) => {
      if (typeof item !== 'object' || item === null) return [];
      if (typeof item.section !== 'string') return [];
      return [{
        section: item.section,
        name: typeof item.name === 'string' && item.name ? item.name : item.section,
        prob: asNumber(item.prob) ?? 0,
      }];
    })
    .slice(0, 64);
}

function asVotes(value) {
  if (!Array.isArray(value)) return [];
  return value
    .flatMap((item) => {
      if (typeof item !== 'object' || item === null) return [];
      if (typeof item.topic !== 'string') return [];
      return [{ topic: item.topic, score: asNumber(item.score) ?? 0 }];
    })
    .slice(0, 64);
}

/** Pure transition: one parsed frame → the next state. */
export function applyStageFrame(state, frame) {
  const base = { ...state, pass: frame.pass, seq: frame.seq, lastStep: frame.step };
  const data = frame.data;

  switch (frame.step) {
    case 'depth':
      return { ...base, depth: data.depth === 'deep' || data.depth === 'simple' ? data.depth : null };

    case 'bm25':
      return { ...base, sectionHits: asNumber(data.sectionHits), poolSize: asNumber(data.poolSize) };

    case 'gate':
      return {
        ...base,
        gateFired: data.fired === true,
        gateRatio: asNumber(data.ratio),
      };

    case 'expansion':
      return {
        ...base,
        escalationTerms: asStringArray(data.terms),
        expansionError: typeof data.error === 'string' ? data.error : null,
      };

    case 'expansionFailed':
      return {
        ...base,
        expansionError: typeof data.message === 'string' ? data.message : 'expansion failed',
      };

    case 'concepts':
      return {
        ...base,
        routingUsed: data.used === true,
        routingModel: typeof data.model === 'string' && data.model ? data.model : null,
        routingCandidates: asNumber(data.candidates),
        routePicks: asConceptPicks(data.picks),
        routingError: typeof data.error === 'string' ? data.error : null,
        routingSecs: asNumber(data.secs),
      };

    case 'elevation':
      return base;

    case 'seeds': {
      const seeds = asStringArray(data.ids);
      return {
        ...base,
        seeds,
        anchors: asStringArray(data.anchors).filter((id) => seeds.includes(id)),
        leads: asStringArray(data.leadIds).filter((id) => seeds.includes(id)),
      };
    }

    case 'votes':
      return { ...base, guides: asStringArray(data.guides), votes: asVotes(data.votes) };

    case 'descent':
      return { ...base, picks: asStringArray(data.pickedOrder) };

    case 'hop': {
      const hop = asNumber(data.hop) ?? state.hops.length + 1;
      const added = asStringArray(data.added);
      const visited = new Set(state.visited);
      for (const id of added) visited.add(id);
      return { ...base, hops: [...state.hops, { hop, added }], visited };
    }

    case 'traversal':
      return { ...base, visitedCount: asNumber(data.visitedCount) };

    case 'scored':
      return { ...base, directHits: asStringArray(data.direct) };

    case 'delivered': {
      const tierOrder = asStringArray(data.tierOrder);
      return {
        ...base,
        tierOrder,
        tierTotal: asNumber(data.tierTotal) ?? tierOrder.length,
        sections: tierOrder,
      };
    }

    case 'reorder': {
      const finalOrder = asSectionRows(data.finalOrder);
      return {
        ...base,
        finalOrder,
        finalTotal: asNumber(data.finalTotal) ?? finalOrder.length,
        sections: finalOrder.map((row) => row.section),
        droppedSections: asStringArray(data.dropped),
        droppedTotal: asNumber(data.droppedTotal) ?? state.droppedTotal,
        reordered: data.reordered !== false,
      };
    }

    case 'evidenceSummary':
      return { ...base, triples: asNumber(data.triples) };

    case 'synthesis':
      return {
        ...base,
        writing: data.phase === 'start',
        writtenChars: data.phase === 'end' ? asNumber(data.chars) : state.writtenChars,
      };

    case 'retry':
      // Pass 2 replays fresh: only the avoidance hint carries over.
      return {
        ...createVizState(),
        pass: 2,
        seq: frame.seq,
        lastStep: 'retry',
        retryActive: true,
        avoidTerms: asStringArray(data.avoidTerms),
      };

    case 'done':
      return { ...base, writing: false, finished: true };

    default:
      // Forward compatibility: unknown steps still advance pass/seq.
      return base;
  }
}

// ── Live store + per-turn capture ─────────────────────────────────────────

let current = createVizState();
let capture = [];

/** Current state snapshot (identity changes on every applied frame). */
export function vizCurrent() {
  return current;
}

/** Reset before a fresh ask or replay. */
export function vizReset() {
  current = createVizState();
  capture = [];
}

/** Feed one parsed frame into the live store and capture it for replay. */
export function vizApplyFrame(frame) {
  current = applyStageFrame(current, frame);
  if (capture.length < 600) capture.push(frame);
  return current;
}

/** Hand over the captured frames of the current turn; later calls get []. */
export function takeCapture() {
  const frames = capture;
  capture = [];
  return frames;
}

/** Free-seek fold: the state after the first `upto` frames (0 = before any). */
export function computeStateAt(frames, upto) {
  let state = createVizState();
  for (let index = 0; index < upto && index < frames.length; index += 1) {
    state = applyStageFrame(state, frames[index]);
  }
  return state;
}
