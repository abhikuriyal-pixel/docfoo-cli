/**
 * Two-layer canvas renderer for the KG scene.
 *
 * The base layer paints the neutral graph (hairline edges, degree-sized
 * nodes, optional hub labels); the top layer paints the query highlight
 * choreography. Both share one world transform, owned here, updated by
 * wheel/drag/pinch gestures. Rendering is demand-driven: nothing runs while
 * the page is idle.
 */

const EDGE_HAIRLINE = 0.75;
/** Above this edge count idle edges stop drawing entirely. */
export const EDGE_CEILING = 30_000;
const LABEL_TOP_K = 12;
const MAX_LABELS = 48;
const ZOOM_MIN = 0.04;
const ZOOM_MAX = 48;
/** Largest scale the automatic fit will choose. */
const FIT_MAX = 1.6;
const PICK_SLOP = 6;
const HOP_DECAY = 0.68;

function cssVar(name, fallback) {
  if (typeof document === 'undefined' || typeof getComputedStyle !== 'function') return fallback;
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

export class VizRenderer {
  /**
   * @param {{ getSpec: () => {cssW:number, cssH:number, dpr:number},
   *           labelsOn: () => boolean }} options
   */
  constructor({ getSpec, labelsOn }) {
    this.getSpec = getSpec;
    this.labelsOn = labelsOn;

    this.graph = null;
    this.layout = null;
    this.baseContext = null;
    this.topContext = null;
    this.highlights = null;
    this.patina = false;
    this.view = { x: 0, y: 0, k: 1 };

    this.pendingRender = false;
    this.dragging = false;
    this.fitted = false;
    this.userAdjusted = false;
    this.onPanState = null;
    this.pointers = new Map();
    this.pinch = null;
    this.downPoint = null;
    this.pickHandler = null;
    this.viewSubscribers = new Set();
    this.labelWidths = new Map();
    this.stats = { baseRebuilds: 0, lastBaseMs: 0 };
  }

  // ── Scene ───────────────────────────────────────────────────────────────

  setScene(graph, layout) {
    this.graph = graph;
    this.layout = layout;
    this.labelWidths.clear();
    this.fitted = false;
    this.userAdjusted = false;
    this.fit();
  }

  clearScene() {
    this.graph = null;
    this.layout = null;
    this.scheduleRender();
  }

  setHighlights(spec) {
    this.highlights = spec;
    this.scheduleRender();
  }

  setPatina(on) {
    if (this.patina === on) return;
    this.patina = on;
    this.scheduleRender();
  }

  dropHighlights() {
    this.highlights = null;
    this.patina = false;
    this.scheduleRender();
  }

  getView() {
    return { ...this.view };
  }

  /**
   * Fit the whole scene — node circles AND their hub-label plates — into the
   * canvas. Labels are screen-constant, so the union bounds are monotone
   * (non-decreasing) in the world scale; a binary search finds the largest
   * scale whose bounds still fit the viewport.
   */
  fit() {
    if (!this.layout || this.layout.width === 0) return;
    const spec = this.getSpec();
    if (!spec || spec.cssW === 0 || spec.cssH === 0) return;
    const margin = 18;
    const availableWidth = spec.cssW - margin * 2;
    const availableHeight = spec.cssH - margin * 2;
    // Label metrics depend on the loaded webfont; a refit after fonts.ready
    // must never reuse widths measured with the fallback face.
    this.labelWidths.clear();
    const fits = (scale) => {
      const bounds = this.screenBounds(scale);
      return (
        bounds.maxX - bounds.minX <= availableWidth
        && bounds.maxY - bounds.minY <= availableHeight
      );
    };

    let scale = FIT_MAX;
    if (!fits(scale)) {
      let low = ZOOM_MIN;
      let high = FIT_MAX;
      for (let pass = 0; pass < 32; pass += 1) {
        const mid = (low + high) / 2;
        if (fits(mid)) low = mid;
        else high = mid;
      }
      scale = low;
    }

    const bounds = this.screenBounds(scale);
    const width = bounds.maxX - bounds.minX;
    const height = bounds.maxY - bounds.minY;
    this.view = {
      k: scale,
      x: (spec.cssW - width) / 2 - bounds.minX,
      y: (spec.cssH - height) / 2 - bounds.minY,
    };
    this.fitted = true;
    this.scheduleRender();
  }

  /** Union bounding box of the graph and its labels at world scale `scale`. */
  screenBounds(scale) {
    const bounds = {
      minX: 0,
      minY: 0,
      maxX: this.layout.width * scale,
      maxY: this.layout.height * scale,
    };
    for (const box of this.labelBoxes()) {
      const x = box.x * scale;
      const y = box.y * scale;
      bounds.minX = Math.min(bounds.minX, x - box.halfWidth);
      bounds.maxX = Math.max(bounds.maxX, x + box.halfWidth);
      bounds.minY = Math.min(bounds.minY, y - box.halfHeight);
      bounds.maxY = Math.max(bounds.maxY, y + box.halfHeight);
    }
    return bounds;
  }

  /** Nodes that receive a hub label (same set drawLabels paints). */
  labelTargets() {
    const hubs = this.layout.hubs || [];
    const topK = [...this.graph.nodes.keys()]
      .sort((a, b) => this.graph.nodes[b][3] - this.graph.nodes[a][3] || a - b)
      .slice(0, LABEL_TOP_K);
    return [...new Set([...hubs, ...topK])].slice(0, MAX_LABELS);
  }

  /** World-space label plates with screen-constant half sizes, for fit(). */
  labelBoxes() {
    const context = this.baseContext;
    const graph = this.graph;
    const layout = this.layout;
    if (!context || !graph || !layout || !this.labelsOn()) return [];
    context.save();
    context.font = "500 11px 'JetBrains Mono', monospace";
    const boxes = [];
    for (const index of this.labelTargets()) {
      const node = graph.nodes[index];
      const point = layout.points[index];
      if (!node || !point) continue;
      const name = node[1];
      let width = this.labelWidths.get(name);
      if (width === undefined) {
        width = context.measureText(name).width;
        this.labelWidths.set(name, width);
      }
      boxes.push({
        x: point.x,
        y: point.y - point.r - 8,
        halfWidth: width / 2 + 4,
        halfHeight: 7.5,
      });
    }
    context.restore();
    return boxes;
  }

  // ── Gestures ────────────────────────────────────────────────────────────

  /**
   * Bind pointer/wheel gestures. Pointer events unify mouse and touch, so
   * drag-pan, pinch-zoom and click-to-pick share one code path.
   */
  attach(element, onPanState) {
    this.onPanState = onPanState || null;
    element.addEventListener('wheel', (event) => this.onWheel(event), { passive: false });
    element.addEventListener('pointerdown', (event) => this.onPointerDown(event));
    element.addEventListener('pointermove', (event) => this.onPointerMove(event));
    element.addEventListener('pointerup', (event) => this.onPointerUp(event));
    element.addEventListener('pointercancel', (event) => this.onPointerUp(event));
    element.addEventListener('dblclick', () => this.fit());
  }

  setPickHandler(handler) {
    this.pickHandler = handler;
  }

  /** Subscribe to every repaint (used to keep the popover glued to a node). */
  onView(callback) {
    this.viewSubscribers.add(callback);
    return () => this.viewSubscribers.delete(callback);
  }

  /** Nearest node to canvas-local coordinates, within a generous reach. */
  pickAt(cssX, cssY) {
    if (!this.layout || !this.graph) return null;
    let best = null;
    let bestDistance = Infinity;
    for (let index = 0; index < this.layout.points.length; index += 1) {
      const point = this.layout.points[index];
      const distance = Math.hypot(
        point.x * this.view.k + this.view.x - cssX,
        point.y * this.view.k + this.view.y - cssY,
      );
      const reach = Math.max(point.r * this.view.k, PICK_SLOP) + 4;
      if (distance <= reach && distance < bestDistance) {
        best = index;
        bestDistance = distance;
      }
    }
    return best;
  }

  onWheel(event) {
    event.preventDefault();
    this.userAdjusted = true;
    this.zoomAt(event.offsetX, event.offsetY, Math.exp(-event.deltaY * 0.0016));
  }

  zoomAt(cx, cy, factor) {
    const previous = this.view;
    const k = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, previous.k * factor));
    const ratio = k / previous.k;
    this.view = {
      k,
      x: cx - (cx - previous.x) * ratio,
      y: cy - (cy - previous.y) * ratio,
    };
    this.scheduleRender();
  }

  onPointerDown(event) {
    this.userAdjusted = true;
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      // The pointer may already be gone (synthetic events in tests).
    }
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });

    if (this.pointers.size === 1 && !this.pinch) {
      this.downPoint = { x: event.clientX, y: event.clientY };
      this.setDragging(true);
    } else {
      this.downPoint = null;
    }

    if (this.pointers.size === 2) {
      const [first, second] = [...this.pointers.values()];
      this.pinch = {
        mid: { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2 },
        distance: Math.hypot(second.x - first.x, second.y - first.y),
      };
      this.setDragging(false);
    }
  }

  onPointerMove(event) {
    const previous = this.pointers.get(event.pointerId);
    if (!previous) return;
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });

    if (this.pointers.size >= 2 && this.pinch) {
      const [first, second] = [...this.pointers.values()];
      const mid = { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2 };
      const distance = Math.hypot(second.x - first.x, second.y - first.y);
      const rect = event.currentTarget.getBoundingClientRect();
      if (this.pinch.distance > 0 && distance > 0) {
        this.zoomAt(mid.x - rect.left, mid.y - rect.top, distance / this.pinch.distance);
      }
      // Two-finger pan rides the midpoint travel.
      this.view.x += mid.x - this.pinch.mid.x;
      this.view.y += mid.y - this.pinch.mid.y;
      this.pinch = { mid, distance };
      this.scheduleRender();
      return;
    }

    if (this.dragging) {
      this.view.x += event.clientX - previous.x;
      this.view.y += event.clientY - previous.y;
      this.scheduleRender();
    }
  }

  onPointerUp(event) {
    this.pointers.delete(event.pointerId);
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      // Already released.
    }
    if (this.pointers.size < 2) this.pinch = null;

    if (this.pointers.size === 0) {
      this.setDragging(false);
      const down = this.downPoint;
      this.downPoint = null;
      if (down && this.pickHandler && (event.button === 0 || event.button === -1)) {
        const travel = Math.hypot(event.clientX - down.x, event.clientY - down.y);
        if (travel < 5) {
          const rect = event.currentTarget.getBoundingClientRect();
          this.pickHandler(this.pickAt(event.clientX - rect.left, event.clientY - rect.top));
        }
      }
    } else if (this.pointers.size === 1) {
      this.setDragging(true);
      this.downPoint = null;
    }
  }

  setDragging(on) {
    if (this.dragging === on) return;
    this.dragging = on;
    if (this.onPanState) this.onPanState(on);
  }

  // ── Painting ────────────────────────────────────────────────────────────

  /** Coalesce burst inputs (gestures, frames) into one microtask paint. */
  scheduleRender() {
    if (this.pendingRender) return;
    this.pendingRender = true;
    queueMicrotask(() => {
      this.pendingRender = false;
      this.drawBase();
      this.drawHighlights();
      for (const callback of [...this.viewSubscribers]) callback();
    });
  }

  /** Theme/resize hook: next paint re-reads tokens and sizes. */
  invalidate() {
    this.labelWidths.clear();
    this.scheduleRender();
  }

  drawBase() {
    const context = this.baseContext;
    if (!context) return;
    const started = typeof performance !== 'undefined' ? performance.now() : Date.now();
    const spec = this.getSpec();
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, context.canvas.width, context.canvas.height);

    if (!this.graph || !this.layout || !spec || spec.cssW === 0 || spec.cssH === 0) {
      this.stats.lastBaseMs = (typeof performance !== 'undefined' ? performance.now() : Date.now()) - started;
      return;
    }

    context.setTransform(spec.dpr, 0, 0, spec.dpr, 0, 0);
    context.translate(this.view.x, this.view.y);
    context.scale(this.view.k, this.view.k);

    const nodeColor = cssVar('--kg-viz-node', '#9a9a92');
    const edgeColor = cssVar('--kg-viz-edge', 'rgba(128,128,128,0.16)');
    const background = cssVar('--background', '#111111');
    const labelBorder = cssVar('--border', '#888');
    const labelInk = cssVar('--foreground', '#eee');
    const points = this.layout.points;

    // Edges first so node fills cover their endpoints.
    if (this.graph.edges.length <= EDGE_CEILING) {
      context.strokeStyle = edgeColor;
      context.lineWidth = EDGE_HAIRLINE;
      context.beginPath();
      for (const [a, b] of this.graph.edges) {
        const from = points[a];
        const to = points[b];
        if (!from || !to) continue;
        context.moveTo(from.x, from.y);
        context.lineTo(to.x, to.y);
      }
      context.stroke();
    }

    // Nodes: degree-sized fill with a background rim so neighbors stay legible.
    context.fillStyle = nodeColor;
    context.beginPath();
    for (const point of points) {
      context.moveTo(point.x + point.r, point.y);
      context.arc(point.x, point.y, point.r, 0, Math.PI * 2);
    }
    context.fill();

    context.strokeStyle = background;
    context.lineWidth = 1.1;
    context.beginPath();
    for (const point of points) {
      context.moveTo(point.x + point.r + 0.55, point.y);
      context.arc(point.x, point.y, point.r + 0.55, 0, Math.PI * 2);
    }
    context.stroke();

    if (this.labelsOn() && this.graph.nodes.length > 0) {
      this.drawLabels(context, points, background, labelBorder, labelInk);
    }

    this.stats.baseRebuilds += 1;
    this.stats.lastBaseMs = (typeof performance !== 'undefined' ? performance.now() : Date.now()) - started;
  }

  /** Hub labels: bright ink on a backing plate, screen-constant size. */
  drawLabels(context, points, background, border, ink) {
    const targets = this.labelTargets();

    context.font = "500 11px 'JetBrains Mono', monospace";
    context.textBaseline = 'middle';
    context.textAlign = 'center';
    for (const index of targets) {
      const node = this.graph.nodes[index];
      const point = points[index];
      if (!node || !point) continue;
      const name = node[1];
      let width = this.labelWidths.get(name);
      if (width === undefined) {
        width = context.measureText(name).width;
        this.labelWidths.set(name, width);
      }
      context.save();
      context.translate(point.x, point.y - point.r - 8);
      context.scale(1 / this.view.k, 1 / this.view.k);
      context.fillStyle = background;
      context.globalAlpha = 0.88;
      context.fillRect(-width / 2 - 4, -7.5, width + 8, 15);
      context.strokeStyle = border;
      context.globalAlpha = 0.35;
      context.strokeRect(-width / 2 - 4, -7.5, width + 8, 15);
      context.globalAlpha = 1;
      context.fillStyle = ink;
      context.fillText(name, 0, 0.5);
      context.restore();
    }
    context.textAlign = 'start';
  }

  drawHighlights() {
    const context = this.topContext;
    if (!context) return;
    const spec = this.getSpec();
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, context.canvas.width, context.canvas.height);
    if (!this.highlights || !this.graph || !this.layout || !spec || spec.cssW === 0 || spec.cssH === 0) {
      return;
    }

    context.setTransform(spec.dpr, 0, 0, spec.dpr, 0, 0);
    context.translate(this.view.x, this.view.y);
    context.scale(this.view.k, this.view.k);

    const seedColor = cssVar('--kg-viz-seed', '#e0b357');
    const hopColor = cssVar('--kg-viz-hop', '#c86a2f');
    const points = this.layout.points;
    // End-of-turn relaxation: highlights keep their place at lower alpha.
    const alpha = this.patina ? 0.32 : 1;

    this.drawGuideClusters(context, points, hopColor, alpha);
    this.drawTrails(context, points, hopColor, alpha);
    this.drawSeeds(context, points, seedColor, alpha);
    this.drawPicks(context, points, hopColor, alpha);
  }

  drawGuideClusters(context, points, color, alpha) {
    const clusters = this.highlights.guideClusters;
    if (clusters.size === 0) return;
    context.strokeStyle = color;
    context.lineWidth = 1;
    context.setLineDash([3, 5]);
    for (const cluster of clusters) {
      const center = this.layout.centers[cluster];
      if (!center) continue;
      let radius = 10;
      for (let index = 0; index < points.length; index += 1) {
        if (this.layout.clusterOf[index] !== cluster) continue;
        const point = points[index];
        radius = Math.max(radius, Math.hypot(point.x - center.x, point.y - center.y) + point.r + 6);
      }
      context.globalAlpha = 0.2 * alpha;
      context.beginPath();
      context.arc(center.x, center.y, radius, 0, Math.PI * 2);
      context.stroke();
    }
    context.setLineDash([]);
    context.globalAlpha = 1;
  }

  /** Hop trails and newly reached nodes, with depth-decayed alpha. */
  drawTrails(context, points, color, alpha) {
    this.highlights.levelEdges.forEach((edges, level) => {
      const levelAlpha = Math.max(0.14, HOP_DECAY ** level) * alpha;
      if (levelAlpha < 0.05 || edges.length === 0) return;
      context.strokeStyle = color;
      context.globalAlpha = levelAlpha;
      context.lineWidth = 1.3;
      context.beginPath();
      for (const [a, b] of edges) {
        const from = points[a];
        const to = points[b];
        if (!from || !to) continue;
        context.moveTo(from.x, from.y);
        context.lineTo(to.x, to.y);
      }
      context.stroke();
    });

    this.highlights.levelAdded.forEach((added, level) => {
      const levelAlpha = Math.max(0.18, HOP_DECAY ** level) * alpha;
      context.fillStyle = color;
      context.globalAlpha = levelAlpha;
      context.beginPath();
      for (const index of added) {
        const point = points[index];
        if (!point) continue;
        context.moveTo(point.x + point.r, point.y);
        context.arc(point.x, point.y, point.r, 0, Math.PI * 2);
      }
      context.fill();
    });
    context.globalAlpha = 1;
  }

  /** Seeds: accent fill + halo; leads wider, anchors ringed. */
  drawSeeds(context, points, color, alpha) {
    for (const index of this.highlights.seeds) {
      const point = points[index];
      if (!point) continue;
      const lead = this.highlights.leads.has(index);
      const anchor = this.highlights.anchors.has(index);
      const haloRadius = point.r + (lead ? 9 : 6);

      context.strokeStyle = color;
      context.globalAlpha = (lead ? 0.55 : 0.38) * alpha;
      context.lineWidth = 1.4;
      context.beginPath();
      context.arc(point.x, point.y, haloRadius, 0, Math.PI * 2);
      context.stroke();

      if (lead) {
        context.globalAlpha = 0.22 * alpha;
        context.beginPath();
        context.arc(point.x, point.y, haloRadius + 4, 0, Math.PI * 2);
        context.stroke();
      }

      context.globalAlpha = alpha;
      context.fillStyle = color;
      context.beginPath();
      context.moveTo(point.x + point.r, point.y);
      context.arc(point.x, point.y, Math.max(1.8, point.r - 0.4), 0, Math.PI * 2);
      context.fill();

      if (anchor) {
        context.strokeStyle = color;
        context.globalAlpha = 0.9 * alpha;
        context.lineWidth = 1.6;
        context.beginPath();
        context.arc(point.x, point.y, point.r + 1.8, 0, Math.PI * 2);
        context.stroke();
      }
    }
    context.globalAlpha = 1;
  }

  /** Guided-descent picks: small markers offset from the node. */
  drawPicks(context, points, color, alpha) {
    context.fillStyle = color;
    context.globalAlpha = 0.85 * alpha;
    for (const index of this.highlights.picks) {
      const point = points[index];
      if (!point || this.highlights.seeds.has(index)) continue;
      context.beginPath();
      context.arc(point.x + point.r + 3.4, point.y - point.r - 3.4, 1.6, 0, Math.PI * 2);
      context.fill();
    }
    context.globalAlpha = 1;
  }
}
