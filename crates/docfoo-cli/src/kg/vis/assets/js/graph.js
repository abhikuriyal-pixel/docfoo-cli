/**
 * Graph projection loading and tolerant wire coercion.
 *
 * The server ships nodes as `[id, name, type, degree]` tuples, edges as
 * node-index pairs and sections as `{title: {t, e}}`. Anything malformed is
 * rejected as a whole rather than half-rendered.
 */

const isIndex = (value, count) => Number.isInteger(value) && value >= 0 && value < count;

function coerceNodes(raw) {
  if (!Array.isArray(raw)) return null;
  const nodes = [];
  for (const entry of raw) {
    if (!Array.isArray(entry) || entry.length !== 4) return null;
    const [id, name, type, degree] = entry;
    if (typeof id !== 'string' || typeof name !== 'string') return null;
    const numericDegree = typeof degree === 'number' ? degree : Number(degree);
    if (!Number.isFinite(numericDegree) || numericDegree < 0) return null;
    nodes.push([id, name, typeof type === 'string' ? type : '', numericDegree]);
  }
  return nodes;
}

function coerceEdges(raw, nodeCount) {
  if (!Array.isArray(raw)) return null;
  const edges = [];
  for (const entry of raw) {
    if (!Array.isArray(entry) || entry.length !== 2) return null;
    const a = typeof entry[0] === 'number' ? entry[0] : Number(entry[0]);
    const b = typeof entry[1] === 'number' ? entry[1] : Number(entry[1]);
    if (!isIndex(a, nodeCount) || !isIndex(b, nodeCount)) return null;
    edges.push([a, b]);
  }
  return edges;
}

function coerceSections(raw, nodeCount) {
  const sections = {};
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return sections;
  for (const [title, value] of Object.entries(raw)) {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) continue;
    sections[title] = {
      t: typeof value.t === 'string' ? value.t : '',
      e: Array.isArray(value.e) ? value.e.filter((index) => isIndex(index, nodeCount)) : [],
    };
  }
  return sections;
}

/** Returns a projection object or null; never throws. */
export function coerceGraph(raw) {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null;
  if (typeof raw.hash !== 'string' || raw.hash.length === 0) return null;
  const nodes = coerceNodes(raw.nodes);
  if (!nodes) return null;
  const edges = coerceEdges(raw.edges, nodes.length);
  if (!edges) return null;

  const descs = Array.isArray(raw.descs)
    ? raw.descs.filter((desc) => typeof desc === 'string')
    : [];
  const noise = typeof raw.noiseFloor === 'number' ? raw.noiseFloor : Number(raw.noiseFloor);

  return {
    hash: raw.hash,
    nodes,
    edges,
    sections: coerceSections(raw.sections, nodes.length),
    noiseFloor: Number.isFinite(noise) ? noise : 0,
    descs: descs.length === nodes.length ? descs : undefined,
  };
}

const projectionCache = new Map();

/**
 * Fetch the projection for a scope. `null` means "no graph for this scope";
 * a transport failure is also null so the shell can show the empty state.
 */
export async function loadGraphData(scope = '') {
  const key = String(scope ?? '');
  try {
    const response = await fetch(`/api/graph?scope=${encodeURIComponent(key)}`, {
      headers: { Accept: 'application/json' },
    });
    if (!response.ok) return null;
    const graph = coerceGraph(await response.json());
    if (!graph) return null;

    const cached = projectionCache.get(key);
    if (cached && cached.hash === graph.hash) return cached.graph;
    projectionCache.set(key, { hash: graph.hash, graph });
    // Bound the cache like the desktop does; the visible graph changes rarely.
    while (projectionCache.size > 3) projectionCache.delete(projectionCache.keys().next().value);
    return graph;
  } catch {
    return null;
  }
}
