/**
 * Pure choreography specs: VizState + graph geometry → exactly what to paint.
 *
 * DOM-free by design: the renderer and shell consume these specs, and the Node
 * test suite can exercise every visual decision without a browser.
 */

/** id → projection index for the current scene. */
export function nodeIndexMap(graph) {
  const map = new Map();
  graph.nodes.forEach((node, index) => map.set(node[0], index));
  return map;
}

const indexesOf = (ids, indexMap) => {
  const result = new Set();
  for (const id of ids) {
    const index = indexMap.get(id);
    if (index !== undefined) result.add(index);
  }
  return result;
};

/**
 * Derive the highlight layer from state. Frontier-crossing edges are computed
 * here (not in the renderer): an edge belongs to hop level L when exactly one
 * endpoint was known before L (seeds ∪ earlier levels) and the other is in
 * L's additions.
 */
export function buildHighlights(state, graph, layout) {
  const indexMap = nodeIndexMap(graph);
  const spec = {
    seeds: indexesOf(state.seeds, indexMap),
    anchors: indexesOf(state.anchors, indexMap),
    leads: indexesOf(state.leads, indexMap),
    levelAdded: [],
    levelEdges: [],
    picks: indexesOf(state.picks, indexMap),
    guideClusters: new Set(),
  };

  const known = new Set(spec.seeds);
  for (const level of state.hops) {
    const added = indexesOf(level.added, indexMap);
    const fresh = [...added].filter((index) => !known.has(index));
    const freshSet = new Set(fresh);
    const crossing = [];
    if (freshSet.size > 0 && graph.edges.length <= 30_000) {
      for (const [a, b] of graph.edges) {
        if (known.has(a) && freshSet.has(b)) crossing.push([a, b]);
        else if (known.has(b) && freshSet.has(a)) crossing.push([b, a]);
      }
    }
    spec.levelAdded.push(new Set(fresh));
    spec.levelEdges.push(crossing);
    for (const index of fresh) known.add(index);
  }

  // Winning guide topics outline their clusters, but only while the graph
  // still has few enough communities for the outlines to read as signal.
  if (state.guides.length > 0 && layout.centers.length <= 24) {
    for (const index of spec.picks) {
      const cluster = layout.clusterOf[index];
      if (cluster >= 0) spec.guideClusters.add(cluster);
    }
    for (const index of spec.seeds) {
      const cluster = layout.clusterOf[index];
      if (cluster >= 0) spec.guideClusters.add(cluster);
    }
  }

  return spec;
}

/** True when the highlight layer has nothing left worth painting. */
export function isEmptyHighlight(spec) {
  return (
    spec.seeds.size === 0
    && spec.picks.size === 0
    && spec.levelAdded.every((level) => level.size === 0)
  );
}

// ── Banners ───────────────────────────────────────────────────────────────

/** Weak-seed bridging announcement (legacy expansion path). */
export function expansionBanner(state) {
  if (!state.escalationTerms.length) return null;
  return {
    msg: 'Weak seeds - bridging lay → technical',
    chips: state.escalationTerms,
    variant: 'accent',
  };
}

/** Pass-1 refusal / pass-2 replay announcement. */
export function retryBanner(state) {
  const terms = state.avoidTerms.slice(0, 4);
  return {
    msg: state.retryActive
      ? 'Pass refused — re-retrieving with forced expansion'
      : 'Re-retrieving with forced expansion',
    chips: terms,
    variant: 'accent',
  };
}

/** Weak-seed Jev concept routing; quiet when routing was off or empty. */
export function routingBanner(state) {
  if (!state.routingUsed || state.routePicks.length === 0) return null;
  return {
    msg: `Concept routing - ${state.routePicks.length} matched`,
    chips: state.routePicks.map((pick) => `${pick.name} ${Math.round(pick.prob * 100)}%`),
    variant: 'accent',
  };
}

// ── Replay pacing ─────────────────────────────────────────────────────────

export const REPLAY_BASE_MS = 380;
export const REPLAY_BUDGET_MS = 15_000;
export const REPLAY_FLOOR_MS = 45;

/** Uniform cadence that compresses long traces into the replay budget. */
export function cadenceFor(frameCount) {
  if (frameCount <= 1) return REPLAY_BASE_MS;
  return Math.min(REPLAY_BASE_MS, Math.max(REPLAY_FLOOR_MS, REPLAY_BUDGET_MS / frameCount));
}

// ── Evidence strip ────────────────────────────────────────────────────────

/** Rows shown before collapsing into a "+N more" tail. */
export const STRIP_CAP = 24;
const DROPPED_CAP = 6;

function markDirect(directTitles, rows) {
  const direct = new Set(directTitles);
  for (const row of rows) row.direct = direct.has(row.title);
}

/** Provisional tier-order view (dimmed) right after `delivered`. */
export function stripForDelivered(tierOrder, tierTotal, directTitles) {
  const shown = tierOrder.slice(0, STRIP_CAP);
  const rows = shown.map((title) => ({ title }));
  markDirect(directTitles, rows);
  return {
    rows,
    hiddenMore: Math.max(0, tierTotal - shown.length),
    total: tierTotal,
    provisional: true,
  };
}

/**
 * Final view after `reorder`: moved rows flash, budget-dropped rows append
 * struck through, and the tail collapses into "+N more".
 */
export function stripForReorder(previousTitles, finalRows, finalTotal, droppedTitles, droppedTotal, directTitles) {
  const previousIndex = new Map();
  previousTitles.forEach((title, index) => previousIndex.set(title, index));

  const shownFinals = finalRows.slice(0, STRIP_CAP);
  const rows = shownFinals.map((row) => ({ title: row.section, chars: row.chars }));
  markDirect(directTitles, rows);

  const shownDrops = Math.min(droppedTitles.length, DROPPED_CAP);
  for (const title of droppedTitles.slice(0, DROPPED_CAP)) {
    rows.push({ title, dropped: true });
  }

  const hiddenMore =
    Math.max(0, finalTotal - shownFinals.length)
    + Math.max(0, droppedTotal - shownDrops);

  const movedRows = new Set();
  shownFinals.forEach((row, index) => {
    const before = previousIndex.get(row.section);
    if (before === undefined || before !== index) movedRows.add(index);
  });

  return {
    view: { rows, hiddenMore, total: finalTotal, provisional: false },
    movedRows,
  };
}
