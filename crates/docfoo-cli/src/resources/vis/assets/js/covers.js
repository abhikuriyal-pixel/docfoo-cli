/**
 * Random cover planning for resource cards — a port of the desktop browser:
 * one random figure per card, a collage of up to four for folders, a monogram
 * for figure-less folders and a kind glyph for plain files. Picks are cached
 * per card and re-rolled by the dice button.
 */

export const MAX_COLLAGE = 4;

/** Fisher–Yates copy so the caller's array is never reordered. */
export function shuffled(list, rng = Math.random) {
  const out = [...list];
  for (let index = out.length - 1; index > 0; index -= 1) {
    const other = Math.floor(rng() * (index + 1));
    [out[index], out[other]] = [out[other], out[index]];
  }
  return out;
}

export function randomOf(list, rng = Math.random) {
  if (!Array.isArray(list) || list.length === 0) return null;
  return list[Math.min(list.length - 1, Math.floor(rng() * list.length))];
}

/** The cover plan for one listing entry. */
export function coverPlan(entry, rng = Math.random) {
  const covers = Array.isArray(entry.covers) ? entry.covers.filter(Boolean) : [];
  if (covers.length > 0) {
    if (entry.kind === 'dir') {
      const figures = shuffled(covers, rng).slice(0, MAX_COLLAGE);
      return { shape: figures.length === 1 ? 'figure' : 'collage', figures };
    }
    return { shape: 'figure', figures: [randomOf(covers, rng)] };
  }
  if (entry.kind === 'dir') return { shape: 'mark', figures: [] };
  return { shape: 'glyph', figures: [], kind: entry.kind };
}

export function coverSignature(plan) {
  return `${plan.shape}:${(plan.figures ?? []).join('|')}`;
}

/** Cache covers per card so re-renders are stable until the dice is pressed. */
export function createCoverPicker(rng = Math.random) {
  const picks = new Map();
  return {
    plan(entry) {
      if (!picks.has(entry.rel)) picks.set(entry.rel, coverPlan(entry, rng));
      return picks.get(entry.rel);
    },
    reroll() {
      picks.clear();
    },
  };
}

export function formatBytes(size) {
  const value = Number(size) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

/** The card's meta line: folder totals or a file's size/line count. */
export function cardMeta(entry) {
  const parts = [];
  if (entry.kind === 'dir') {
    if (entry.files) parts.push(`${entry.files} file${entry.files === 1 ? '' : 's'}`);
    if (entry.md) parts.push(`${entry.md} doc${entry.md === 1 ? '' : 's'}`);
    if (entry.figures) parts.push(`${entry.figures} figure${entry.figures === 1 ? '' : 's'}`);
  } else {
    parts.push(formatBytes(entry.size));
    if (entry.lines) parts.push(`${entry.lines} lines`);
  }
  return parts.join(' · ');
}

export function kindLabel(kind) {
  switch (kind) {
    case 'dir':
      return 'Folder';
    case 'md':
      return 'Markdown';
    case 'image':
      return 'Image';
    case 'notebook':
      return 'Notebook';
    default:
      return 'File';
  }
}
