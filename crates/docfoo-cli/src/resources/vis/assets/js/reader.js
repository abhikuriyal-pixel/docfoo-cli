/**
 * Document reader — the block-based pipeline ported from the desktop app's
 * windowed reader: one `.reader-block` wrapper per marked top-level block
 * carrying its exact source line, plus the desktop's image-resolution rules
 * (candidate paths, downscale-free asset route, natural-width tracking) and
 * the per-block KaTeX auto-render pass for OCR math.
 */

import {
  imageCandidates,
  lexReaderBlocks,
  outlineFromBlocks,
  renderBlock,
  renderMath,
  trackNaturalWidth,
} from './md-doc.js';

/**
 * Render `markdown` into `root`. Returns the lexed blocks and the
 * `{ level, text, line }` outline derived from them.
 */
export function renderDocument(root, markdown, baseDir) {
  const blocks = lexReaderBlocks(markdown);
  root.textContent = '';
  for (const block of blocks) {
    const element = document.createElement('div');
    element.className = 'reader-block';
    element.dataset.kind = block.kind;
    element.dataset.line = String(block.line);
    element.innerHTML = renderBlock(block);
    root.appendChild(element);
    resolveBlockImages(element, baseDir);
    renderMath(element);
  }
  return { blocks, outline: outlineFromBlocks(blocks) };
}

function imageBasename(source) {
  const path = String(source ?? '').split(/[?#]/, 1)[0] ?? '';
  let decoded = path.replace(/^file:(?:\/\/)?/i, '');
  try {
    decoded = decodeURIComponent(decoded);
  } catch {
    // Keep the raw value when it is not valid percent-encoding.
  }
  return decoded.replace(/\\/g, '/').split('/').pop() ?? decoded;
}

/**
 * Point every unresolved `<img>` at `/api/asset`, trying the desktop's
 * candidate list in order (document folder, then bare basename for absolute
 * paths) and tracking the decoded natural width for the figure slider.
 */
export function resolveBlockImages(root, dir) {
  for (const img of root.querySelectorAll('img')) {
    if (img.dataset.resolved === '1') continue;
    img.dataset.resolved = '1';
    const source = img.getAttribute('src') ?? '';
    if (!source || /^(data:|https?:|mailto:)/i.test(source)) {
      trackNaturalWidth(img);
      continue;
    }
    const candidates = imageCandidates(source, dir);
    if (candidates.length === 0) continue;
    img.removeAttribute('src');
    img.dataset.src = imageBasename(source) || candidates[0];
    img.classList.add('res-img-loading');
    loadCandidate(img, candidates, 0);
  }
}

function loadCandidate(img, candidates, index) {
  if (index >= candidates.length) {
    img.classList.remove('res-img-loading');
    img.classList.add('res-img-error');
    return;
  }
  const candidate = candidates[index];
  img.dataset.resRel = candidate;

  const onLoad = () => {
    img.removeEventListener('error', onError);
    if (img.naturalWidth > 0 && img.naturalHeight > 0) {
      img.style.aspectRatio = `${img.naturalWidth} / ${img.naturalHeight}`;
    }
    trackNaturalWidth(img);
    img.classList.remove('res-img-loading');
  };
  const onError = () => {
    img.removeEventListener('load', onLoad);
    loadCandidate(img, candidates, index + 1);
  };
  img.addEventListener('load', onLoad, { once: true });
  img.addEventListener('error', onError, { once: true });
  img.src = `/api/asset?path=${encodeURIComponent(candidate)}`;
}

/** Build the outline list; returns the item elements for active tracking. */
export function renderToc(list, outline, onPick) {
  list.textContent = '';
  for (const heading of outline) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = `toc-item lvl-${tocIndent(heading.level)}`;
    button.textContent = heading.text || `Line ${heading.line}`;
    button.title = heading.text;
    button.dataset.line = String(heading.line);
    button.addEventListener('click', () => onPick(heading.line));
    list.appendChild(button);
  }
  return [...list.querySelectorAll('.toc-item')];
}

function tocIndent(level) {
  return Math.min(3, Math.max(1, Number(level) || 1));
}

/** The exact block for a source line, else the nearest block above it. */
export function blockForLine(root, line) {
  let best = null;
  let bestLine = -1;
  for (const block of root.querySelectorAll('[data-line]')) {
    const value = Number(block.dataset.line);
    if (!Number.isFinite(value)) continue;
    if (value === line) return block;
    if (value <= line && value > bestLine) {
      best = block;
      bestLine = value;
    }
  }
  return best;
}

/** Highlight the outline item for the block currently at the top. */
export function watchOutline(scroller, root, outline, onChange) {
  const marks = outline
    .map((heading) => ({ line: heading.line, element: blockForLine(root, heading.line) }))
    .filter((mark) => mark.element);
  if (marks.length === 0) return () => {};

  let frame = 0;
  const update = () => {
    frame = 0;
    const threshold = scroller.scrollTop + 90;
    let current = marks[0].line;
    for (const mark of marks) {
      if (mark.element.offsetTop <= threshold) current = mark.line;
    }
    onChange(current);
  };
  const onScroll = () => {
    if (!frame) frame = requestAnimationFrame(update);
  };
  scroller.addEventListener('scroll', onScroll, { passive: true });
  update();
  return () => {
    scroller.removeEventListener('scroll', onScroll);
    if (frame) cancelAnimationFrame(frame);
  };
}
