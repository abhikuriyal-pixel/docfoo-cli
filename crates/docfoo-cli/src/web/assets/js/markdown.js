/**
 * Compact, XSS-safe markdown renderer shared by the KG and resource viewers.
 *
 * Supports headings, paragraphs, bold/italic, inline code, fenced code,
 * lists, blockquotes, horizontal rules, links, GFM tables, figures, citation
 * chips and KaTeX math. Raw HTML is always escaped before it reaches the
 * page; generated elements are slotted before escaping so their markup
 * survives.
 *
 * Math handling mirrors the desktop app (`src/lib/markdown.ts`): `$…$` /
 * `$$…$$` (plus `\(…\)`, `\[…\]` and standalone math environments) are
 * extracted BEFORE block parsing as inert private-use placeholders, rendered
 * with KaTeX once the HTML is assembled. Inline code is protected in the same
 * pass, so `$` inside backticks never becomes math.
 */

export function escapeHtml(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** Resource figure path → loopback asset route (http(s)/data URLs pass through).
 *  `baseDir` is the document's folder, so `assets/f.png` in `Doc/content.md`
 *  resolves to `Doc/assets/f.png`. */
export function rewriteFigureSrc(path, baseDir = '') {
  const clean = String(path ?? '').trim().replace(/\\/g, '/').replace(/^\.\//, '');
  if (!clean) return '';
  if (/^(?:https?:|data:|blob:)/i.test(clean)) return clean;
  return `/api/asset?path=${encodeURIComponent(joinRel(baseDir, clean))}`;
}

/** Resolve `path` against `baseDir`, collapsing `.`/`..` segments. */
function joinRel(baseDir, path) {
  const parts = String(baseDir ?? '')
    .split('/')
    .filter((segment) => segment && segment !== '.');
  for (const segment of String(path).split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      parts.pop();
      continue;
    }
    parts.push(segment);
  }
  return parts.join('/');
}

// ── KaTeX math (port of the desktop's protect/restore pipeline) ────────────

/** Delimiters KaTeX auto-render understands (kept for parity/tests). */
export const KATEX_DELIMITERS = [
  { left: '$$', right: '$$', display: true },
  { left: '$', right: '$', display: false },
  { left: '\\(', right: '\\)', display: false },
];

/** Math environments KaTeX renders as a unit, e.g. \begin{align}…\end{align}. */
const MATH_ENVIRONMENTS = [
  'equation', 'equation*', 'align', 'align*', 'alignat', 'alignat*',
  'gather', 'gather*', 'multline', 'multline*', 'displaymath', 'math',
  'array', 'cases', 'matrix', 'pmatrix', 'bmatrix', 'Bmatrix',
  'vmatrix', 'Vmatrix', 'split', 'aligned', 'gathered', 'smallmatrix',
];

const MATH_ENV_RE = new RegExp(
  String.raw`\\begin\{(${MATH_ENVIRONMENTS.map((env) => env.replace(/\*/g, '\\*')).join('|')})\}([\s\S]*?)\\end\{\1\}`,
  'g',
);

/** A bracket/paren span holding only numbers/punctuation is a citation
 *  (`\[12, 36, 37\]`), not math: leave it for the text pass. */
const CITATION_ONLY = /^[\s\d,;.+\-–—()[\]A-Z._:&/-]*$/;

/** `$$\n\[ x \]\tag{1}\n$$` (a common PDF-to-markdown shape) → `x \tag{1}`. */
function unwrapDisplayBrackets(tex) {
  return tex.replace(/^\s*\\\[/, '').replace(/\\\]/g, '');
}

/**
 * Render one expression to HTML. `globalThis.katex` is set by the vendored
 * `katex.min.js` script in the page; without it (Node tests, missing asset)
 * the span degrades to escaped literal text.
 */
export function renderMathToString(tex, display) {
  const fallback = display ? `$$${tex}$$` : `$${tex}$`;
  const katex = globalThis.katex;
  if (!katex || typeof katex.renderToString !== 'function') return escapeHtml(fallback);
  try {
    return katex.renderToString(tex, {
      throwOnError: false,
      displayMode: display,
      errorColor: 'currentColor',
    });
  } catch {
    return escapeHtml(fallback);
  }
}

const MATH_MARKER = /\uE000KM(\d+)\uE000/g;
const CODE_MARKER = /\uE000KC(\d+)\uE000/g;

/**
 * Replace fenced blocks (untouched), inline code and math spans with inert
 * private-use placeholders. Fenced chunks are kept in place so the block
 * parser still sees them; math/code are restored after rendering.
 */
export function protectMathAndCode(source, options = {}) {
  const math = [];
  const code = [];
  const chunks = source.split(/(```[\s\S]*?```|~~~[\s\S]*?~~~)/g);

  const out = chunks.map((chunk, index) => {
    if (index % 2 === 1) return chunk; // fenced code block

    // Inline code first, so `$` inside backticks never becomes math. Newlines
    // inside a span are preserved after the marker so protected text keeps
    // the same line numbering as the source (the `sourceLines` option).
    let text = chunk.replace(/(`+)([\s\S]*?)\1/g, (whole, _ticks, body) => {
      const id = code.push(body) - 1;
      return `\uE000KC${id}\uE000${'\n'.repeat((whole.match(/\n/g) || []).length)}`;
    });
    // Reader-grade callers repair escaped HTML tags here, after inline code is
    // protected and before math extraction (desktop `protectMathSpans`).
    if (typeof options.repair === 'function') text = options.repair(text);

    const addMath = (tex, display, raw = '') => {
      const id = math.push({ tex, display }) - 1;
      return `\uE000KM${id}\uE000${'\n'.repeat((raw.match(/\n/g) || []).length)}`;
    };

    // Display math takes precedence over inline; environments are extracted
    // last so environments inside a delimiter stay part of their math.
    text = text
      .replace(/(?<!\\)\$\$([\s\S]*?)\$\$/g, (whole, tex) => addMath(unwrapDisplayBrackets(tex), true, whole))
      .replace(/(?<!\\)\\\[([\s\S]*?)\\\]/g, (whole, tex) =>
        (CITATION_ONLY.test(tex) ? whole : addMath(tex, true, whole)))
      .replace(/(?<!\\)\\\(([\s\S]*?)\\\)/g, (whole, tex) =>
        (CITATION_ONLY.test(tex) ? whole : addMath(tex, false, whole)))
      .replace(/(?<!\\)\$(?!\s)((?:\\.|[^$\\\n])+?)(?<!\s)\$/g, (_, tex) => addMath(tex, false))
      .replace(MATH_ENV_RE, (whole, env, body) =>
        addMath(`\\begin{${env}}${body}\\end{${env}}`, env !== 'math', whole));

    return text;
  });

  return { text: out.join(''), math, code };
}

export function restoreMathAndCode(html, math, code) {
  return html
    .replace(MATH_MARKER, (_, index) => {
      const span = math[Number(index)];
      return span ? renderMathToString(span.tex, span.display) : '';
    })
    .replace(CODE_MARKER, (_, index) => `<code>${escapeHtml(code[Number(index)] ?? '')}</code>`);
}

// ── Inline markdown ────────────────────────────────────────────────────────

const IMAGE_PATTERN = /!\[([^\]]*)\]\(\s*(?:<([^>]+)>|([^)]+?))\s*\)/g;
const LINK_PATTERN = /\[([^\]]+)\]\(\s*(?:<([^>]+)>|([^\s)]+))(?:\s+(?:"[^"]*"|'[^']*'|\([^)]*\)))?\s*\)/g;
/** `[doc/content.md:12-34]` (and plain document names in brackets). */
export const CITATION_PATTERN = /\[([A-Za-z0-9_][A-Za-z0-9_\-./\\ ,]*\.(?:md|markdown|pdf|txt|json|csv|html?)(?::[0-9\-, ]+)?)\]/g;

function safeHref(value) {
  const url = String(value ?? '').trim();
  return /^(?:javascript|vbscript|data):/i.test(url) ? '#' : url;
}

/**
 * One inline pass. Each generated element is parked in a slot and replaced by
 * a control-character marker; the remaining prose is escaped, then the slots
 * are restored. Math/code placeholders from the protect pass pass through
 * untouched.
 */
export function renderInline(raw, options = {}) {
  const slots = [];
  const slot = (html) => {
    const marker = `\u0001${slots.length}\u0002`;
    slots.push(html);
    return marker;
  };

  let text = String(raw ?? '');

  text = text.replace(IMAGE_PATTERN, (whole, alt, anglePath, plainPath) => {
    const path = (anglePath ?? plainPath ?? '').trim();
    const src = rewriteFigureSrc(path, options.baseDir);
    if (!src) return whole;
    return slot(
      `<img class="kg-figure" src="${escapeHtml(src)}" alt="${escapeHtml(alt)}" loading="lazy">`,
    );
  });

  text = text.replace(LINK_PATTERN, (_, label, angleUrl, plainUrl) => {
    const href = safeHref(angleUrl ?? plainUrl ?? '');
    const external = /^(?:https?:|mailto:)/i.test(href);
    return slot(
      `<a href="${escapeHtml(href)}"${external ? ' target="_blank" rel="noreferrer"' : ''}>${escapeHtml(label)}</a>`,
    );
  });

  text = text.replace(CITATION_PATTERN, (whole, citation) => {
    const label = citation.trim();
    if (!label) return whole;
    return slot(`<code class="citation-chip">${escapeHtml(label)}</code>`);
  });

  const escaped = escapeHtml(text)
    .replace(/\*\*([^*\n]+?)\*\*/g, '<strong>$1</strong>')
    .replace(/__([^_\n]+?)__/g, '<strong>$1</strong>')
    .replace(/(?<!\*)\*([^*\n]+?)\*(?!\*)/g, '<em>$1</em>')
    .replace(/(?<!_)_([^_\n]+?)_(?!_)/g, '<em>$1</em>');

  return escaped.replace(/\u0001(\d+)\u0002/g, (_, index) => slots[Number(index)] ?? '');
}

// ── Block parsing ──────────────────────────────────────────────────────────

function splitTableRow(line) {
  let value = String(line ?? '').trim();
  if (value.startsWith('|')) value = value.slice(1);
  if (value.endsWith('|') && !value.endsWith('\\|')) value = value.slice(0, -1);

  const cells = [];
  let current = '';
  let escaping = false;
  for (const character of value) {
    if (character === '|' && !escaping) {
      cells.push(current.trim().replace(/\\\|/g, '|'));
      current = '';
      continue;
    }
    current += character;
    escaping = character === '\\' && !escaping;
    if (character !== '\\') escaping = false;
  }
  cells.push(current.trim().replace(/\\\|/g, '|'));
  return cells;
}

function isTableSeparator(line) {
  const cells = splitTableRow(line);
  return cells.length > 0 && cells.every((cell) => /^:?-+:?$/.test(cell));
}

function isTableStart(lines, index) {
  return Boolean(lines[index]?.includes('|'))
    && index + 1 < lines.length
    && isTableSeparator(lines[index + 1]);
}

const FENCE = /^\s*(```+|~~~+)\s*([^\s]*)?/;
const HEADING = /^\s{0,3}(#{1,6})\s+(.+?)\s*#*\s*$/;
const HR = /^\s*(?:(?:\*\s*){3,}|(?:-\s*){3,}|_{3,})\s*$/;
const LIST_ITEM = /^\s*(?:([-+*])|(\d+)[.)])\s+(.*)$/;
const QUOTE = /^\s*>/;

function isBlockStart(lines, index) {
  const line = lines[index] ?? '';
  return FENCE.test(line)
    || HEADING.test(line)
    || HR.test(line)
    || QUOTE.test(line)
    || LIST_ITEM.test(line)
    || isTableStart(lines, index);
}

function renderFence(lines, start, output, options) {
  const marker = FENCE.exec(lines[start]);
  const fenceChar = marker[1][0];
  const language = (marker[2] || '').replace(/[^A-Za-z0-9_-]/g, '');
  const closer = new RegExp(`^\\s*${fenceChar}{3,}`);
  const body = [];
  let index = start + 1;
  while (index < lines.length && !closer.test(lines[index])) {
    body.push(lines[index]);
    index += 1;
  }
  if (index < lines.length) index += 1;
  const className = language ? ` class="language-${escapeHtml(language)}"` : '';
  output.push(`<pre${lineAttr(start, options)}><code${className}>${escapeHtml(body.join('\n'))}</code></pre>`);
  return index;
}

/** `data-line` for a block that starts on 0-based `index` of `lines`. */
function lineAttr(index, options) {
  if (!options || !options.sourceLines) return '';
  return ` data-line="${(options.lineOffset || 0) + index + 1}"`;
}

function renderTable(lines, start, output, options) {
  const header = splitTableRow(lines[start]);
  const body = [];
  let index = start + 2;
  while (index < lines.length && lines[index].includes('|') && lines[index].trim() !== '') {
    body.push(splitTableRow(lines[index]));
    index += 1;
  }
  const head = header.map((cell) => `<th>${renderInline(cell, options)}</th>`).join('');
  const rows = body
    .map((row) => `<tr>${header.map((_, column) => `<td>${renderInline(row[column] ?? '', options)}</td>`).join('')}</tr>`)
    .join('');
  output.push(`<table${lineAttr(start, options)}><thead><tr>${head}</tr></thead><tbody>${rows}</tbody></table>`);
  return index;
}

function renderList(lines, start, output, options) {
  const ordered = /^\s*\d+[.)]\s+/.test(lines[start]);
  const tag = ordered ? 'ol' : 'ul';
  const items = [];
  let index = start;
  while (index < lines.length && LIST_ITEM.test(lines[index])) {
    const match = LIST_ITEM.exec(lines[index]);
    items.push(`<li${lineAttr(index, options)}>${renderInline(match[3], options)}</li>`);
    index += 1;
  }
  output.push(`<${tag}${lineAttr(start, options)}>${items.join('')}</${tag}>`);
  return index;
}

function renderBlocks(lines, options = {}) {
  const output = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (line.trim() === '') {
      index += 1;
      continue;
    }
    if (FENCE.test(line)) {
      index = renderFence(lines, index, output, options);
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      const level = heading[1].length;
      output.push(`<h${level}${lineAttr(index, options)}>${renderInline(heading[2], options)}</h${level}>`);
      index += 1;
      continue;
    }
    if (HR.test(line)) {
      output.push(`<hr${lineAttr(index, options)}>`);
      index += 1;
      continue;
    }
    if (isTableStart(lines, index)) {
      index = renderTable(lines, index, output, options);
      continue;
    }
    if (QUOTE.test(line)) {
      const start = index;
      const quote = [];
      while (index < lines.length && QUOTE.test(lines[index])) {
        quote.push(lines[index].replace(/^\s*>\s?/, ''));
        index += 1;
      }
      output.push(`<blockquote${lineAttr(start, options)}>${renderBlocks(quote, {
        ...options,
        lineOffset: (options.lineOffset || 0) + start,
      })}</blockquote>`);
      continue;
    }
    if (LIST_ITEM.test(line)) {
      index = renderList(lines, index, output, options);
      continue;
    }

    const start = index;
    const paragraph = [line];
    index += 1;
    while (
      index < lines.length
      && lines[index].trim() !== ''
      && !isBlockStart(lines, index)
    ) {
      paragraph.push(lines[index]);
      index += 1;
    }
    output.push(`<p${lineAttr(start, options)}>${renderInline(paragraph.join('\n'), options).replace(/\n/g, '<br>\n')}</p>`);
  }
  return output.join('\n');
}

export function renderMarkdown(markdown, options = {}) {
  const text = String(markdown ?? '').replace(/\r\n?/g, '\n');
  const { text: protectedText, math, code } = protectMathAndCode(text);
  const html = renderBlocks(protectedText.split('\n'), options);
  return restoreMathAndCode(html, math, code);
}
