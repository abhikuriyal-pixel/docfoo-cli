import test from 'node:test';
import assert from 'node:assert/strict';

import { renderMarkdown, rewriteFigureSrc } from '../../src/kg/vis/assets/js/markdown.js';

test('raw HTML is escaped before formatting', () => {
  const html = renderMarkdown('Hello <script>alert("x")</script> **bold**');
  assert.ok(!html.includes('<script>'), html);
  assert.ok(html.includes('&lt;script&gt;'));
  assert.ok(html.includes('<strong>bold</strong>'));
});

test('citations become code chips rather than markdown links', () => {
  const html = renderMarkdown('See [doc/content.md:12-34] and [notes.txt].');
  assert.ok(html.includes('<code class="kg-cite">doc/content.md:12-34</code>'), html);
  assert.ok(html.includes('<code class="kg-cite">notes.txt</code>'));

  const link = renderMarkdown('[doc/content.md:12-34](https://example.com)');
  assert.ok(link.includes('<a href="https://example.com"'), link);
  assert.ok(!link.includes('kg-cite'), link);
});

test('figures rewrite to the loopback asset endpoint and paths are decoded', () => {
  assert.equal(rewriteFigureSrc('Book/assets/f 1.png'), '/api/asset?path=Book%2Fassets%2Ff%201.png');
  assert.equal(rewriteFigureSrc('https://example.com/f.png'), 'https://example.com/f.png');

  const html = renderMarkdown('![Chart](Book/assets/f%201.png)');
  assert.ok(html.includes('class="kg-figure"'), html);
  assert.ok(html.includes('src="/api/asset?path=Book%2Fassets%2Ff%25201.png"'), html);
  assert.ok(html.includes('alt="Chart"'));
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

test('inline code protects markdown-looking content', () => {
  const html = renderMarkdown('Use `[not/a/link.md]` literally');
  assert.ok(html.includes('<code class="kg-inline-code">[not/a/link.md]</code>'), html);
  assert.ok(!html.includes('kg-cite'), html);
});
