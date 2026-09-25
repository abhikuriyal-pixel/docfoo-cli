/**
 * The document markdown pipeline — a direct port of the desktop app's
 * `src/lib/markdown.ts` plus the block model from
 * `src/features/resources/reader/blocks.ts`:
 *
 *   protect math/inline code → repair OCR escapes → marked (GFM)
 *   → DOMPurify sanitize → restore KaTeX + code
 *
 * Documents are lexed once with `marked.lexer` into top-level blocks that
 * carry their exact 1-based source line, so the outline and highlight notes
 * anchor to the same positions as the desktop reader.
 *
 * `kg --vis` keeps its own compact renderer (`markdown.js`); this module is
 * the reader-grade one used by `resources --vis`.
 */

import { marked } from '../vendor/marked.esm.js';
import {
  escapeHtml,
  protectMathAndCode,
  restoreMathAndCode,
} from './markdown.js';
import { repairEscapedHtmlTags, repairSpacedImageDestinations } from './md-repair.js';

/** Tag/attribute allowlist for resource markdown (desktop parity: scripts,
 *  event handlers, styles, iframes and forms are all excluded). */
const SANITIZE_OPTIONS = {
  ALLOWED_TAGS: [
    'a', 'b', 'blockquote', 'br', 'code', 'del', 'details', 'div', 'em',
    'figcaption', 'figure', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr',
    'i', 'img', 'input', 'kbd', 'li', 'ol', 'p', 'pre', 's', 'span',
    'strong', 'sub', 'summary', 'sup', 'table', 'tbody', 'td', 'tfoot',
    'th', 'thead', 'tr', 'ul',
  ],
  ALLOWED_ATTR: [
    'alt', 'checked', 'class', 'colspan', 'disabled', 'height', 'href',
    'id', 'lang', 'rel', 'rowspan', 'src', 'start', 'target', 'title',
    'type', 'width',
  ],
  ALLOW_DATA_ATTR: false,
};

// External links open in the browser, never replacing the viewer page.
let hookReady = false;

function sanitize(html) {
  const purifier = globalThis.DOMPurify;
  if (!purifier || typeof purifier.sanitize !== 'function') return html;
  if (!hookReady && typeof purifier.addHook === 'function') {
    hookReady = true;
    purifier.addHook('afterSanitizeAttributes', (node) => {
      if (node instanceof Element && node.tagName === 'A' && node.getAttribute('href')) {
        node.setAttribute('target', '_blank');
        node.setAttribute('rel', 'noopener noreferrer');
      }
    });
  }
  return purifier.sanitize(html, SANITIZE_OPTIONS);
}

/** Render ONE top-level markdown fragment to sanitized HTML. */
export function renderMarkdownFragment(raw) {
  try {
    const { text, math, code } = protectMathAndCode(raw, { repair: repairEscapedHtmlTags });
    const html = marked.parse(repairSpacedImageDestinations(text), { gfm: true });
    return restoreMathAndCode(sanitize(html), math, code);
  } catch {
    return `<pre>${escapeHtml(raw)}</pre>`;
  }
}

/** Render a whole document in one pass (used by tests and fallbacks). */
export function renderMarkdownDocument(raw) {
  try {
    const { text, math, code } = protectMathAndCode(raw, { repair: repairEscapedHtmlTags });
    const html = marked.parse(repairSpacedImageDestinations(text), { gfm: true });
    return restoreMathAndCode(sanitize(html), math, code);
  } catch {
    return `<pre>${escapeHtml(raw)}</pre>`;
  }
}

// ── Block model (port of reader/blocks.ts) ─────────────────────────────────

/** Code fences in the desktop reader are sliced at this many lines. */
export const CODE_CHUNK_LINES = 200;

function countNewlines(value) {
  let count = 0;
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === '\n') count += 1;
  }
  return count;
}

function blockKind(type) {
  switch (type) {
    case 'heading':
      return 'heading';
    case 'list':
      return 'list';
    case 'table':
      return 'table';
    case 'blockquote':
      return 'quote';
    case 'html':
      return 'html';
    case 'hr':
      return 'hr';
    default:
      return 'paragraph';
  }
}

function isComment(raw) {
  return /^<!--[\s\S]*?-->$/.test(raw.trim());
}

/** A code token's body without its fence lines, as one reader block. */
function codeBlock(raw, lang, at, line) {
  const lines = raw.split('\n');
  const open = /^\s*(```+|~~~+)/.exec(lines[0] ?? '');
  let bodyStart = 0;
  let bodyEnd = lines.length;
  if (open) {
    bodyStart = 1;
    if (bodyEnd > bodyStart && lines[bodyEnd - 1] === '') bodyEnd -= 1;
    const fenceChar = open[1][0];
    const closer = new RegExp(`^\\s*${fenceChar === '`' ? '`' : '~'}{${open[1].length},}\\s*$`);
    if (bodyEnd > bodyStart && closer.test(lines[bodyEnd - 1])) bodyEnd -= 1;
  } else if (bodyEnd > 0 && lines[bodyEnd - 1] === '') {
    bodyEnd -= 1;
  }
  return {
    kind: 'code',
    source: lines.slice(bodyStart, bodyEnd).join('\n'),
    start: at,
    end: at + raw.length,
    line,
    lang: lang || '',
  };
}

/**
 * Lex `content` into top-level blocks with exact source ranges and lines.
 * Unlocatable tokens abort the walk quietly: everything located so far
 * stays valid.
 */
export function lexReaderBlocks(content) {
  const source = String(content ?? '');
  const blocks = [];
  let tokens;
  try {
    tokens = marked.lexer(source);
  } catch {
    return [{ kind: 'html', source, start: 0, end: source.length, line: 1 }];
  }
  let cursor = 0;
  let line = 1;
  for (const token of tokens) {
    const raw = token.raw ?? '';
    if (!raw) continue;
    const at = source.indexOf(raw, cursor);
    if (at < 0) break;
    line += countNewlines(source.slice(cursor, at));
    cursor = at + raw.length;
    if (token.type === 'space' || isComment(raw)) {
      line += countNewlines(raw);
      continue;
    }
    if (token.type === 'code') {
      blocks.push(codeBlock(raw, token.lang, at, line));
    } else {
      blocks.push({
        kind: blockKind(token.type),
        source: raw,
        start: at,
        end: cursor,
        line,
        level: token.depth,
      });
    }
    line += countNewlines(raw);
  }
  return blocks;
}

/** Rendered HTML for one block. Code is plain monospace (no highlighter). */
export function renderBlock(block) {
  if (block.kind === 'code') {
    const className = block.lang ? ` class="language-${escapeHtml(block.lang)}"` : '';
    return `<pre><code${className}>${escapeHtml(block.source)}</code></pre>`;
  }
  return renderMarkdownFragment(block.source);
}

/** Markdown → plain text, good enough for the outline (desktop port). */
export function stripMarkdown(source) {
  return String(source ?? '')
    .replace(/<[^>]+>/g, ' ')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^\s{0,3}#{1,6}\s+/gm, '')
    .replace(/^\s*>\s?/gm, '')
    .replace(/^\s*(?:[-*+]|\d+[.)])\s+/gm, '')
    .replace(/`{1,3}/g, '')
    .replace(/[*_~]{1,3}/g, '')
    .replace(/\|/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

export function blockPlainText(block) {
  return block.kind === 'code' ? block.source : stripMarkdown(block.source);
}

/** Outline entries from lexed blocks: `{ level, text, line }`. */
export function outlineFromBlocks(blocks) {
  return blocks
    .filter((block) => block.kind === 'heading')
    .map((block) => ({
      level: block.level ?? 1,
      text: blockPlainText(block),
      line: block.line,
    }));
}

/** Index of the last block starting at or before `line`. */
export function findBlockByLine(blocks, line) {
  let low = 0;
  let high = blocks.length - 1;
  let best = 0;
  while (low <= high) {
    const mid = (low + high) >> 1;
    if (blocks[mid].line <= line) {
      best = mid;
      low = mid + 1;
    } else {
      high = mid - 1;
    }
  }
  return best;
}

// ── Image resolution (port of viewer-images.ts) ────────────────────────────

function imageBasename(source) {
  const path = String(source ?? '').split(/[?#]/, 1)[0] ?? String(source ?? '');
  let decoded = path.replace(/^file:(?:\/\/)?/i, '');
  try {
    decoded = decodeURIComponent(decoded);
  } catch {
    // Keep the raw value when it is not valid percent-encoding.
  }
  return decoded.replace(/\\/g, '/').split('/').pop() ?? decoded;
}

/** Candidate resource paths for an `<img src>`, resolved against the doc dir. */
export function imageCandidates(source, dir) {
  const clean = String(source ?? '').split(/[?#]/, 1)[0] ?? String(source ?? '');
  const isWindowsAbsolute = /^[a-z]:[\\/]/i.test(clean);
  const isFileUrl = /^file:/i.test(clean);
  if (isWindowsAbsolute || isFileUrl) {
    const basename = imageBasename(source);
    if (!basename) return [];
    return [...new Set([dir ? `${dir}/${basename}` : basename, basename])];
  }
  let relPath = clean.replace(/\\/g, '/').replace(/^\.\//, '');
  try {
    relPath = decodeURIComponent(relPath);
  } catch {
    // Keep the raw value when it is not valid percent-encoding.
  }
  if (!relPath) return [];
  return [dir ? `${dir}/${relPath}` : relPath];
}

/** Figure slider support: remember the decoded natural width in `--img-w`. */
export function trackNaturalWidth(img) {
  const apply = () => {
    if (img.naturalWidth > 0) img.style.setProperty('--img-w', `${img.naturalWidth}px`);
  };
  if (img.complete && img.naturalWidth > 0) apply();
  else img.addEventListener('load', apply, { once: true });
}

/** KaTeX auto-render over a mounted block (OCR keeps `\( \)` / `\[ \]`). */
export function renderMath(element) {
  const autoRender = globalThis.renderMathInElement;
  if (typeof autoRender !== 'function') return;
  try {
    autoRender(element, {
      delimiters: [
        { left: '$$', right: '$$', display: true },
        { left: '$', right: '$', display: false },
        { left: '\\(', right: '\\)', display: false },
      ],
      throwOnError: false,
      errorColor: 'currentColor',
    });
  } catch (error) {
    console.warn('KaTeX render error:', error);
  }
}
