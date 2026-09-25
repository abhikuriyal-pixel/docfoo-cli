/**
 * Deterministic KG layout engine.
 *
 * Pure function: projection in → positions out. Identical input yields
 * identical output — no forces, no randomness, no iteration after placement.
 *
 *   1. Communities — label propagation (<= 12 sweeps, in-place updates, ties
 *      resolve to the lower community key so the result is scan-stable).
 *   2. Hubs — each community's highest-degree member is pinned at its center
 *      with a radius boost.
 *   3. Degenerate fallback — when a graph is one giant community or mostly
 *      singletons, nodes go on concentric global rings instead of clusters.
 *   4. Clusters — communities sorted by summed degree, centers placed on a
 *      ring (extra rings rotate by the golden angle).
 *   5. Members — packed onto concentric rings whose radii are computed from
 *      the node diameters plus a fixed edge gap, so circles never overlap.
 *   6. Radii — ~√degree clamped to [2.5, 9]; hubs boost up to 12.
 *
 * Output coordinates live in a padded positive quadrant, so the renderer can
 * treat world space as [0..width] x [0..height].
 */

/** Minimum gap between circle edges on the same ring (world px). */
const EDGE_GAP = 7;
/** Extra radial clearance between consecutive ring outlines. */
const RING_CLEARANCE = 6;
const RADIUS_MIN = 2.5;
const RADIUS_MAX = 9;
const RADIUS_HUB_MAX = 12;
const GOLDEN_ANGLE = 2.39996323;
/** A graph at/above this singleton ratio skips cluster placement. */
const DEGENERATE_RATIO = 0.7;
const MAX_LABEL_SWEEPS = 12;
const CLUSTERS_PER_RING = 16;

const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

function baseRadius(degree) {
  return clamp(RADIUS_MIN + 2.35 * Math.sqrt(degree), RADIUS_MIN, RADIUS_MAX);
}

function hubRadius(radius) {
  return clamp(
    Math.max(radius * 1.4, RADIUS_MAX + 2),
    RADIUS_HUB_MAX - 0.5,
    RADIUS_HUB_MAX,
  );
}

/**
 * Synchronous label propagation. Neighbor order is edge-insertion order, and
 * equal-count ties pick the lower label, so the partition is deterministic.
 */
function propagate(nodeCount, neighbors) {
  const labels = Array.from({ length: nodeCount }, (_, index) => index);

  for (let sweep = 0; sweep < MAX_LABEL_SWEEPS; sweep += 1) {
    let changed = false;
    for (let node = 0; node < nodeCount; node += 1) {
      const counts = new Map();
      for (const neighbor of neighbors[node]) {
        const label = labels[neighbor];
        counts.set(label, (counts.get(label) || 0) + 1);
      }
      let bestLabel = labels[node];
      let bestCount = 0;
      for (const [label, count] of counts) {
        if (count > bestCount || (count === bestCount && count > 0 && label < bestLabel)) {
          bestLabel = label;
          bestCount = count;
        }
      }
      if (bestCount > 0 && bestLabel !== labels[node]) {
        labels[node] = bestLabel;
        changed = true;
      }
    }
    if (!changed) break;
  }
  return labels;
}

/**
 * Radius-aware ring packing. `members` excludes the hub already placed at
 * (cx, cy); `centerRadius` is that hub's radius so the first ring clears it.
 * Angular slots are proportional to diameter, keeping edge-to-edge clearance
 * uniform around each ring.
 */
function packRadial(members, cx, cy, radii, points, centerRadius) {
  let cursor = 0;
  let previousRingRadius = 0;
  let previousRingMaxRadius = centerRadius;

  while (cursor < members.length) {
    const ringMembers = [];
    let sumArc = 0;
    let maxRadius = 0;
    const firstRadius = radii[members[cursor]];
    let ringRadius = Math.max(
      firstRadius + EDGE_GAP,
      previousRingRadius + previousRingMaxRadius + firstRadius + RING_CLEARANCE,
    );

    while (cursor < members.length) {
      const member = members[cursor];
      const radius = radii[member];
      const neededArc = sumArc + 2 * radius + EDGE_GAP * (ringMembers.length + 1);
      const neededRadius = neededArc / (Math.PI * 2);
      const clearOfPrevious =
        previousRingRadius + previousRingMaxRadius + radius + RING_CLEARANCE;
      if (ringMembers.length > 0 && neededRadius > Math.max(ringRadius, clearOfPrevious)) {
        break;
      }
      ringMembers.push(member);
      sumArc = neededArc;
      maxRadius = Math.max(maxRadius, radius);
      ringRadius = Math.max(ringRadius, neededRadius);
      cursor += 1;
    }

    if (ringMembers.length === 0) {
      // Defensive: a single oversized member always gets its own ring.
      ringMembers.push(members[cursor]);
      maxRadius = radii[members[cursor]];
      ringRadius = Math.max(ringRadius, maxRadius + EDGE_GAP);
      cursor += 1;
    }

    let angleCursor = EDGE_GAP / 2;
    for (const member of ringMembers) {
      const halfDiameter = radii[member];
      const angle = ((angleCursor + halfDiameter) / sumArc) * Math.PI * 2;
      points[member] = {
        x: cx + ringRadius * Math.cos(angle),
        y: cy + ringRadius * Math.sin(angle),
        r: radii[member],
      };
      angleCursor += radii[member] * 2 + EDGE_GAP;
    }

    previousRingRadius = ringRadius;
    previousRingMaxRadius = maxRadius;
  }
}

/** Shift every point/center into a padded positive quadrant. */
function normalize(points, clusterOf, centers, hubs, degenerate) {
  const maxRadius = points.reduce((max, point) => Math.max(max, point.r), 0);
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const point of points) {
    minX = Math.min(minX, point.x - point.r);
    minY = Math.min(minY, point.y - point.r);
    maxX = Math.max(maxX, point.x + point.r);
    maxY = Math.max(maxY, point.y + point.r);
  }
  const padding = maxRadius + 4;
  const dx = -minX + padding;
  const dy = -minY + padding;
  for (const point of points) {
    point.x += dx;
    point.y += dy;
  }
  for (const center of centers) {
    center.x += dx;
    center.y += dy;
  }
  return {
    points,
    clusterOf,
    centers,
    hubs,
    width: maxX - minX + padding * 2,
    height: maxY - minY + padding * 2,
    degenerate,
  };
}

/**
 * Layout a graph projection. Never throws on odd input: empty graphs yield an
 * empty result; sparse/disconnected graphs take the concentric fallback.
 */
export function computeLayout(graph) {
  const count = graph.nodes.length;
  if (count === 0) {
    return {
      points: [],
      clusterOf: [],
      centers: [],
      hubs: [],
      width: 0,
      height: 0,
      degenerate: true,
    };
  }

  const degrees = graph.nodes.map((node) => node[3]);
  const radii = degrees.map(baseRadius);

  const neighbors = Array.from({ length: count }, () => []);
  for (const [a, b] of graph.edges) {
    if (a === b) continue;
    neighbors[a].push(b);
    neighbors[b].push(a);
  }

  const labels = propagate(count, neighbors);
  const groups = new Map();
  for (let index = 0; index < count; index += 1) {
    const group = groups.get(labels[index]);
    if (group) group.push(index);
    else groups.set(labels[index], [index]);
  }

  const communities = [...groups.entries()];
  const singletonRatio =
    communities.filter(([, members]) => members.length === 1).length / communities.length;
  const degenerate = communities.length === 1 || singletonRatio >= DEGENERATE_RATIO;

  const points = new Array(count);
  const clusterOf = new Array(count).fill(-1);
  const centers = [];
  const hubs = [];
  const byImportance = (a, b) => degrees[b] - degrees[a] || a - b;

  if (degenerate) {
    const order = [...graph.nodes.keys()].sort(byImportance);
    const hub = order[0];
    hubs.push(hub);
    radii[hub] = hubRadius(radii[hub]);
    points[hub] = { x: 0, y: 0, r: radii[hub] };
    packRadial(order.slice(1), 0, 0, radii, points, radii[hub]);
    return normalize(points, clusterOf, centers, hubs, true);
  }

  // Heaviest communities at the front; ties by lowest member index.
  communities.sort((a, b) => {
    const massA = a[1].reduce((sum, index) => sum + degrees[index], 0);
    const massB = b[1].reduce((sum, index) => sum + degrees[index], 0);
    return massB - massA || a[1][0] - b[1][0];
  });

  const communityCount = communities.length;
  const averageMembers = count / communityCount;
  const centerGap = Math.max(120, 2.9 * Math.sqrt(averageMembers) * (RADIUS_MAX + EDGE_GAP));
  const baseRingRadius =
    communityCount === 1 ? 0 : Math.max(150, (communityCount * centerGap) / (Math.PI * 2));

  communities.forEach(([, members], communityIndex) => {
    const ringIndex = Math.floor(communityIndex / CLUSTERS_PER_RING);
    const angleStep = (Math.PI * 2) / Math.min(communityCount, CLUSTERS_PER_RING);
    const angle = (communityIndex % CLUSTERS_PER_RING) * angleStep + ringIndex * GOLDEN_ANGLE;
    const ringRadius = baseRingRadius + ringIndex * centerGap * 1.15;
    const cx = baseRingRadius === 0 ? 0 : Math.cos(angle) * ringRadius;
    const cy = baseRingRadius === 0 ? 0 : Math.sin(angle) * ringRadius;
    centers.push({ x: cx, y: cy, mass: members.reduce((sum, index) => sum + degrees[index], 0) });

    const order = [...members].sort(byImportance);
    const hub = order[0];
    hubs.push(hub);
    radii[hub] = hubRadius(radii[hub]);
    points[hub] = { x: cx, y: cy, r: radii[hub] };
    for (const member of order) clusterOf[member] = communityIndex;
    packRadial(order.slice(1), cx, cy, radii, points, radii[hub]);
  });

  return normalize(points, clusterOf, centers, hubs, false);
}
