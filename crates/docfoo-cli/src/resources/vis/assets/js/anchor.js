/**
 * Note payloads and ordering — the desktop schema is
 * `id/type/text/originalText/filePath/created/anchor{startLine,startCol,
 * endLine,endCol}/sourceLine`.
 *
 * The browser stores the 1-based rendered-block line hint (`data-line`) plus
 * the selected quote; the desktop re-anchors by quote when offsets drift, and
 * the server fills in `filePath` and the creation stamp.
 */

export function newNoteId(now = Date.now(), random = Math.random) {
  const suffix = Math.floor(random() * 1e6).toString(36);
  return `ann-${now.toString(36)}-${suffix}`;
}

export function buildNote({ id, text, quote, line, start = 0, end = 0, created = Date.now() }) {
  const sourceLine = Math.max(1, Number(line) || 1);
  return {
    id,
    type: 'NOTE',
    text: String(text ?? ''),
    originalText: String(quote ?? ''),
    filePath: '',
    created,
    anchor: {
      startLine: sourceLine - 1,
      startCol: Number(start) || 0,
      endLine: sourceLine - 1,
      endCol: Number(end) || 0,
    },
    sourceLine,
  };
}

/** Panel order: document position first, then creation time. Notes without a
 *  known line sort last. */
export function sortNotes(notes) {
  return [...(notes ?? [])].sort((a, b) => {
    const line = (noteLine(a) || Infinity) - (noteLine(b) || Infinity);
    if (line !== 0) return line;
    const created = (Number(a?.created) || 0) - (Number(b?.created) || 0);
    if (created !== 0) return created;
    return String(a?.id ?? '').localeCompare(String(b?.id ?? ''));
  });
}

/** 1-based source line of a note, or 0 when unknown. */
export function noteLine(note) {
  const line = Number(note?.sourceLine);
  if (Number.isFinite(line) && line > 0) return line;
  const anchorLine = Number(note?.anchor?.startLine);
  return Number.isFinite(anchorLine) ? anchorLine + 1 : 0;
}

/** Character offsets of the current selection inside its rendered block. */
export function textOffsetWithin(block, node, offset) {
  if (!block || !node) return 0;
  let total = 0;
  const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    if (walker.currentNode === node) return total + offset;
    total += walker.currentNode.textContent.length;
  }
  return total;
}

/** The block element (`data-line`) that owns a selection node. */
export function owningBlock(node, root) {
  const element = node?.nodeType === Node.ELEMENT_NODE ? node : node?.parentElement;
  return element?.closest('[data-line]') ?? root ?? null;
}
