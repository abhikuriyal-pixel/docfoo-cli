/**
 * Compact, XSS-safe markdown renderer for KG answers.
 *
 * Supports headings, paragraphs, bold/italic, inline code, fenced code,
 * lists, blockquotes, horizontal rules, links, GFM tables, figures and
 * citation chips. Raw HTML is always escaped before it reaches the page;
 * generated elements are slotted before escaping so their markup survives.
 */

export function escapeHtml(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** Resource figure path → loopback asset route (http(s) URLs pass through). */
export function rewriteFigureSrc(path) {
  const clean = String(path ?? '').trim().replace(/\\/g, '/').replace(/^\.\//, '');
  if (!clean) return '';
  if (/^https?:\/\//i.test(clean)) return clean;
  return `/api/asset?path=${encodeURIComponent(clean)}`;
}

const IMAGE_PATTERN = /!\[([^\]]*)\]\(\s*(?:<([^>]+)>|([^)]+?))\s*\)/g;
const LINK_PATTERN = /\[([^\]]+)\]\(\s*(?:<([^>]+)>|([^\s)]+))(?:\s+(?:"[^"]*"|'[^']*'|\([^)]*\)))?\s*\)/g;
const CODE_PATTERN = /`([^`\n]+)`/g;
/** `[doc/content.md:12-34]` (and plain document names in brackets). */
export const CITATION_PATTERN = /\[([A-Za-z0-9_][A-Za-z0-9_\-./\\ ,]*\.(?:md|markdown|pdf|txt|json|csv|html?)(?::[0-9\-, ]+)?)\]/g;

function safeHref(value) {
  const url = String(value ?? '').trim();
  return /^(?:javascript|vbscript|data):/i.test(url) ? '#' : url;
}

/**
 * One inline pass. Each generated element is parked in a slot and replaced by
 * a control-character marker; the remaining prose is escaped, then the slots
 * are restored. Footnote: inline code runs first so markdown-looking text
 * inside backticks is never reformatted.
 */
export function renderInline(raw) {
  const slots = [];
  const slot = (html) => {
    const marker = `\u0001${slots.length}\u0002`;
    slots.push(html);
    return marker;
  };

  let text = String(raw ?? '');

  text = text.replace(CODE_PATTERN, (_, code) =>
    slot(`<code class="kg-inline-code">${escapeHtml(code)}</code>`));

  text = text.replace(IMAGE_PATTERN, (_, alt, anglePath, plainPath) => {
    const path = (anglePath ?? plainPath ?? '').trim();
    const src = rewriteFigureSrc(path);
    if (!src) return _;
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
    return slot(`<code class="kg-cite">${escapeHtml(label)}</code>`);
  });

  const escaped = escapeHtml(text)
    .replace(/\*\*([^*\n]+?)\*\*/g, '<strong>$1</strong>')
    .replace(/__([^_\n]+?)__/g, '<strong>$1</strong>')
    .replace(/(?<!\*)\*([^*\n]+?)\*(?!\*)/g, '<em>$1</em>')
    .replace(/(?<!_)_([^_\n]+?)_(?!_)/g, '<em>$1</em>');

  return escaped.replace(/\u0001(\d+)\u0002/g, (_, index) => slots[Number(index)] ?? '');
}

// ── Block parsing ─────────────────────────────────────────────────────────

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

function renderFence(lines, start, output) {
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
  output.push(`<pre><code${className}>${escapeHtml(body.join('\n'))}</code></pre>`);
  return index;
}

function renderTable(lines, start, output) {
  const header = splitTableRow(lines[start]);
  const body = [];
  let index = start + 2;
  while (index < lines.length && lines[index].includes('|') && lines[index].trim() !== '') {
    body.push(splitTableRow(lines[index]));
    index += 1;
  }
  const head = header.map((cell) => `<th>${renderInline(cell)}</th>`).join('');
  const rows = body
    .map((row) => `<tr>${header.map((_, column) => `<td>${renderInline(row[column] ?? '')}</td>`).join('')}</tr>`)
    .join('');
  output.push(`<table><thead><tr>${head}</tr></thead><tbody>${rows}</tbody></table>`);
  return index;
}

function renderList(lines, start, output) {
  const ordered = /^\s*\d+[.)]\s+/.test(lines[start]);
  const tag = ordered ? 'ol' : 'ul';
  const items = [];
  let index = start;
  while (index < lines.length && LIST_ITEM.test(lines[index])) {
    const match = LIST_ITEM.exec(lines[index]);
    items.push(`<li>${renderInline(match[3])}</li>`);
    index += 1;
  }
  output.push(`<${tag}>${items.join('')}</${tag}>`);
  return index;
}

function renderBlocks(lines) {
  const output = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (line.trim() === '') {
      index += 1;
      continue;
    }
    if (FENCE.test(line)) {
      index = renderFence(lines, index, output);
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      const level = heading[1].length;
      output.push(`<h${level}>${renderInline(heading[2])}</h${level}>`);
      index += 1;
      continue;
    }
    if (HR.test(line)) {
      output.push('<hr>');
      index += 1;
      continue;
    }
    if (isTableStart(lines, index)) {
      index = renderTable(lines, index, output);
      continue;
    }
    if (QUOTE.test(line)) {
      const quote = [];
      while (index < lines.length && QUOTE.test(lines[index])) {
        quote.push(lines[index].replace(/^\s*>\s?/, ''));
        index += 1;
      }
      output.push(`<blockquote>${renderBlocks(quote)}</blockquote>`);
      continue;
    }
    if (LIST_ITEM.test(line)) {
      index = renderList(lines, index, output);
      continue;
    }

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
    output.push(`<p>${renderInline(paragraph.join('\n')).replace(/\n/g, '<br>\n')}</p>`);
  }
  return output.join('\n');
}

export function renderMarkdown(markdown) {
  const text = String(markdown ?? '').replace(/\r\n?/g, '\n');
  return renderBlocks(text.split('\n'));
}
