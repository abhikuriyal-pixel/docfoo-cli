import test from 'node:test';
import assert from 'node:assert/strict';

import {
  imageCandidates,
  lexReaderBlocks,
  outlineFromBlocks,
  renderBlock,
  renderMarkdownFragment,
  stripMarkdown,
} from '../../src/web/assets/js/md-doc.js';
import {
  repairEscapedHtmlTags,
  repairSpacedImageDestinations,
} from '../../src/web/assets/js/md-repair.js';

// These fixtures mirror the OCR markdown shapes that broke the old regex
// renderer: long bracket-heavy alt text, escaped stars and spaced paths.

test('long bracket-heavy alt text still renders as an image', () => {
  const markdown = '![The image displays a table. \\*\\*Structure:\\*\\* \\* Main Column 1: names \\[24\\]: data](assets/table/p02_01.jpg)';
  const html = renderMarkdownFragment(markdown);
  assert.ok(html.includes('<img'), html);
  assert.ok(html.includes('src="assets/table/p02_01.jpg"'), html);
  assert.ok(html.includes('alt="The image displays a table.'), html);
});

test('escaped stars stay literal instead of becoming emphasis', () => {
  const html = renderMarkdownFragment('handled \\*without input resampling\\*, and more');
  assert.ok(html.includes('*without input resampling*'), html);
  assert.ok(!html.includes('<em>'), html);
  // Unescaped stars still emphasise.
  assert.ok(renderMarkdownFragment('*emphasis*').includes('<em>emphasis</em>'));
});

test('math placeholders survive marked and restore without KaTeX', () => {
  const html = renderMarkdownFragment('The value $x^2$ is inline.\n\n$$\nE = mc^2\n$$');
  assert.ok(html.includes('$x^2$'), html);
  assert.ok(html.includes('E = mc^2'), html);
});

test('spaced image destinations are wrapped before parsing', () => {
  assert.equal(
    repairSpacedImageDestinations('![Figure](assets/table/p02 01.jpg)'),
    '![Figure](<assets/table/p02 01.jpg>)',
  );
  assert.equal(
    repairSpacedImageDestinations('![F](<assets/a b.png>)'),
    '![F](<assets/a b.png>)',
  );
  assert.equal(
    repairSpacedImageDestinations('![F](assets/a.png)'),
    '![F](assets/a.png)',
  );
  const html = renderMarkdownFragment('![Figure](assets/table/p02 01.jpg)');
  // marked percent-encodes the space; imageCandidates decodes it again.
  assert.ok(html.includes('src="assets/table/p02%2001.jpg"'), html);
  assert.deepEqual(
    imageCandidates('assets/table/p02%2001.jpg', 'Book'),
    ['Book/assets/table/p02 01.jpg'],
  );
});

test('escaped HTML tags are repaired so the markup renders', () => {
  const repaired = repairEscapedHtmlTags('\\<table\\>\\<tr\\>\\<td\\>x\\</td\\>\\</tr\\>\\</table\\>');
  assert.equal(repaired, '<table><tr><td>x</td></tr></table>');
  const html = renderMarkdownFragment('\\<table\\>\\<tr\\>\\<td\\>x\\</td\\>\\</tr\\>\\</table\\>');
  assert.ok(html.includes('<table>'), html);
  assert.ok(html.includes('<td>x</td>'), html);
});

test('lexReaderBlocks records kinds and exact source lines', () => {
  const markdown = [
    '# Title',
    '',
    'Intro paragraph',
    '',
    '- one',
    '- two',
    '',
    '```js',
    'const a = 1;',
    '```',
    '',
    '| A |',
    '| --- |',
    '| 1 |',
    '',
    '> quote',
  ].join('\n');
  const blocks = lexReaderBlocks(markdown);
  assert.deepEqual(
    blocks.map((block) => [block.kind, block.line]),
    [
      ['heading', 1],
      ['paragraph', 3],
      ['list', 5],
      ['code', 8],
      ['table', 12],
      ['quote', 16],
    ],
  );
  assert.equal(blocks[0].level, 1);
  assert.equal(blocks[3].lang, 'js');
  assert.equal(blocks[3].source, 'const a = 1;');
  assert.deepEqual(outlineFromBlocks(blocks), [{ level: 1, text: 'Title', line: 1 }]);
});

test('renderBlock renders headings, tables and plain code', () => {
  const blocks = lexReaderBlocks('# Title\n\n| A |\n| --- |\n| 1 |\n\n```sh\necho hi\n```\n');
  const html = blocks.map(renderBlock).join('\n');
  assert.ok(html.includes('<h1>Title</h1>'), html);
  assert.ok(html.includes('<table>'), html);
  assert.ok(html.includes('<pre><code class="language-sh">echo hi</code></pre>'), html);
});

test('image candidates follow the desktop resolution rules', () => {
  assert.deepEqual(imageCandidates('assets/f.png', 'Book'), ['Book/assets/f.png']);
  assert.deepEqual(imageCandidates('./assets/f 1.png', 'Book'), ['Book/assets/f 1.png']);
  assert.deepEqual(imageCandidates('C:\\scans\\figure.jpg', 'Book'), ['Book/figure.jpg', 'figure.jpg']);
  assert.deepEqual(imageCandidates('file:///tmp/f.png', 'Book'), ['Book/f.png', 'f.png']);
  assert.deepEqual(imageCandidates('', 'Book'), []);
  assert.deepEqual(imageCandidates('https://example.com/f.png', 'Book'), ['Book/https://example.com/f.png']);
});

test('stripMarkdown keeps outline labels readable', () => {
  assert.equal(stripMarkdown('## **Bold** and `code`'), 'Bold and code');
  assert.equal(outlineFromBlocks(lexReaderBlocks('## `A` [B](x.md)'))[0].text, 'A B');
});
