import test from 'node:test';
import assert from 'node:assert/strict';

import { renderMarkdown, rewriteFigureSrc } from '../../src/web/assets/js/markdown.js';

/** Minimal KaTeX stand-in so math extraction can be asserted without a DOM. */
function withFakeKatex(run) {
  const calls = [];
  globalThis.katex = {
    renderToString(tex, options) {
      calls.push({ tex, display: options.displayMode });
      return `<span class="katex" data-tex="${tex}" data-display="${options.displayMode}">K</span>`;
    },
  };
  try {
    run(calls);
  } finally {
    delete globalThis.katex;
  }
}

test('raw HTML is escaped before formatting', () => {
  const html = renderMarkdown('Hello <script>alert("x")</script> **bold**');
  assert.ok(!html.includes('<script>'), html);
  assert.ok(html.includes('&lt;script&gt;'));
  assert.ok(html.includes('<strong>bold</strong>'));
});

test('citations become primary-coloured chips rather than markdown links', () => {
  const html = renderMarkdown('See [doc/content.md:12-34] and [notes.txt].');
  assert.ok(html.includes('<code class="citation-chip">doc/content.md:12-34</code>'), html);
  assert.ok(html.includes('<code class="citation-chip">notes.txt</code>'));

  const link = renderMarkdown('[doc/content.md:12-34](https://example.com)');
  assert.ok(link.includes('<a href="https://example.com"'), link);
  assert.ok(!link.includes('citation-chip'), link);
});

test('figures rewrite to the loopback asset endpoint and paths are decoded', () => {
  assert.equal(rewriteFigureSrc('Book/assets/f 1.png'), '/api/asset?path=Book%2Fassets%2Ff%201.png');
  assert.equal(rewriteFigureSrc('https://example.com/f.png'), 'https://example.com/f.png');

  const html = renderMarkdown('![Chart](Book/assets/f%201.png)');
  assert.ok(html.includes('class="kg-figure"'), html);
  assert.ok(html.includes('src="/api/asset?path=Book%2Fassets%2Ff%25201.png"'), html);
  assert.ok(html.includes('alt="Chart"'));
});

test('figures resolve relative to the document folder when baseDir is given', () => {
  assert.equal(rewriteFigureSrc('assets/f.png', 'Book'), '/api/asset?path=Book%2Fassets%2Ff.png');
  assert.equal(rewriteFigureSrc('./assets/f.png', 'a/b'), '/api/asset?path=a%2Fb%2Fassets%2Ff.png');
  assert.equal(rewriteFigureSrc('../shared/f.png', 'a/b'), '/api/asset?path=a%2Fshared%2Ff.png');
  assert.equal(rewriteFigureSrc('https://example.com/f.png', 'Book'), 'https://example.com/f.png');

  const html = renderMarkdown('![Fig](assets/f.png)', { baseDir: 'Book' });
  assert.ok(html.includes('src="/api/asset?path=Book%2Fassets%2Ff.png"'), html);
  // Tables and lists inherit the same base.
  const table = renderMarkdown('| A |\n| --- |\n| ![F](assets/t.png) |', { baseDir: 'Dir' });
  assert.ok(table.includes('/api/asset?path=Dir%2Fassets%2Ft.png'), table);
});

test('sourceLines stamps data-line for notes and the outline', () => {
  const markdown = [
    '# Title',
    '',
    'Para one',
    'still one',
    '',
    '```js',
    '# not a heading',
    '```',
    '',
    '> quote',
    '',
    '| A |',
    '| --- |',
    '| 1 |',
  ].join('\n');
  const html = renderMarkdown(markdown, { sourceLines: true });
  assert.ok(html.includes('<h1 data-line="1">'), html);
  assert.ok(html.includes('<p data-line="3">'), html);
  assert.ok(html.includes('<pre data-line="6">'), html);
  assert.ok(html.includes('<blockquote data-line="10">'), html);
  assert.ok(html.includes('<table data-line="12">'), html);
  // The default output stays free of line stamps.
  assert.ok(!renderMarkdown(markdown).includes('data-line'));
});

test('math and code spanning lines keep later block lines stable', () => {
  const markdown = ['$$', 'x', '$$', '', 'After', '', '`a', 'b`'].join('\n');
  const html = renderMarkdown(markdown, { sourceLines: true });
  assert.ok(html.includes('<p data-line="5">'), html);
  assert.ok(html.includes('<p data-line="7">'), html);
});

test('tables, lists, code fences and headings render as blocks', () => {
  const markdown = [
    '# Title',
    '',
    '| A | B |',
    '| --- | --- |',
    '| 1 | 2 |',
    '',
    '- one',
    '- two',
    '',
    '> quoted',
    '',
    '```rust',
    'let x = 1;',
    '```',
  ].join('\n');
  const html = renderMarkdown(markdown);
  assert.ok(html.includes('<h1>Title</h1>'));
  assert.ok(html.includes('<table><thead><tr><th>A</th><th>B</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>'));
  assert.ok(html.includes('<ul><li>one</li><li>two</li></ul>'));
  assert.ok(html.includes('<blockquote><p>quoted</p></blockquote>'));
  assert.ok(html.includes('<pre><code class="language-rust">let x = 1;</code></pre>'));
});

test('inline code protects markdown and math-looking content', () => {
  const html = renderMarkdown('Use `[not/a/link.md]` and `$not math$` literally');
  assert.ok(html.includes('<code>[not/a/link.md]</code>'), html);
  assert.ok(html.includes('<code>$not math$</code>'), html);
  assert.ok(!html.includes('citation-chip'), html);
});

test('inline and display math are extracted and rendered by KaTeX', () => {
  withFakeKatex((calls) => {
    const html = renderMarkdown('Inline $x^2$ here.\n\n$$\\frac{a}{b}$$\n\nMore.');
    assert.equal(calls.length, 2);
    assert.deepEqual(calls[0], { tex: 'x^2', display: false });
    assert.deepEqual(calls[1], { tex: '\\frac{a}{b}', display: true });
    assert.ok(html.includes('data-display="false"'), html);
    assert.ok(html.includes('data-display="true"'), html);
    assert.ok(!html.includes('$x^2$'), 'math source must not leak as text');
  });
});

test('math environments render and fenced code is never math', () => {
  withFakeKatex((calls) => {
    const html = renderMarkdown('Before\n\n\\begin{align}\na &= b \\\\\n\\end{align}\n\n```\n$not math$ x^2\n```');
    assert.equal(calls.length, 1);
    assert.ok(calls[0].display);
    assert.ok(html.includes('data-tex="\\begin{align}'), html);
    assert.ok(html.includes('$not math$ x^2'), 'fenced code stays literal');
  });
});

test('math degrades to escaped literal text when KaTeX is unavailable', () => {
  const html = renderMarkdown('Inline $x^2$ and \\(y\\).');
  assert.ok(html.includes('$x^2$'), html);
  assert.ok(!html.includes('<span class="katex"'), html);
});
