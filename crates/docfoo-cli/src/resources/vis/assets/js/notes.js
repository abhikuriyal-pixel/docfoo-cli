/**
 * Highlight notes: selection toolbar, note popover, in-document highlights
 * and the notes panel. Notes are the only thing this page ever writes, and
 * they go through the desktop app's `notes.json` schema.
 */

import {
  buildNote,
  newNoteId,
  noteLine,
  owningBlock,
  sortNotes,
  textOffsetWithin,
} from './anchor.js';

export function createNoteController({
  body,
  toolbar,
  popover,
  popoverTitle,
  popoverQuote,
  popoverError,
  textarea,
  list,
  count,
  empty,
  notify,
  api,
}) {
  let rel = '';
  let notes = [];
  let pending = null;
  let editing = null;

  // ── Panel ────────────────────────────────────────────────────────────────

  function renderPanel() {
    list.textContent = '';
    count.textContent = notes.length > 0 ? ` (${notes.length})` : '';
    empty.hidden = notes.length > 0;
    for (const note of notes) {
      const card = document.createElement('article');
      card.className = 'note-card';
      card.dataset.noteId = note.id;

      const text = document.createElement('p');
      text.className = 'note-text';
      text.textContent = note.text || '(empty note)';
      card.appendChild(text);

      if (note.originalText) {
        const quote = document.createElement('p');
        quote.className = 'note-quote';
        quote.textContent = note.originalText;
        card.appendChild(quote);
      }

      const actions = document.createElement('div');
      actions.className = 'note-actions';
      const edit = document.createElement('button');
      edit.type = 'button';
      edit.textContent = 'Edit';
      edit.addEventListener('click', (event) => {
        event.stopPropagation();
        beginEdit(note.id);
      });
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.className = 'danger';
      remove.textContent = 'Delete';
      remove.addEventListener('click', async (event) => {
        event.stopPropagation();
        await removeNote(note.id);
      });
      actions.append(edit, remove);
      card.appendChild(actions);

      card.addEventListener('click', () => focusNote(note.id));
      list.appendChild(card);
    }
  }

  // ── Highlights ───────────────────────────────────────────────────────────

  function unwrapHighlights() {
    for (const span of [...body.querySelectorAll('span.annotation-highlight')]) {
      const parent = span.parentNode;
      while (span.firstChild) parent.insertBefore(span.firstChild, span);
      parent.removeChild(span);
    }
  }

  /** Every text node in document order, with its start offset in the body. */
  function textMap() {
    const nodes = [];
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
    let length = 0;
    while (walker.nextNode()) {
      nodes.push({ node: walker.currentNode, start: length });
      length += walker.currentNode.textContent.length;
    }
    return { nodes, length };
  }

  function locate(nodes, index) {
    for (let i = nodes.length - 1; i >= 0; i -= 1) {
      const { node, start } = nodes[i];
      if (index >= start) {
        return { node, offset: Math.min(index - start, node.textContent.length) };
      }
    }
    return null;
  }

  function wrapRange(start, end, id) {
    const range = document.createRange();
    range.setStart(start.node, start.offset);
    range.setEnd(end.node, end.offset);
    if (range.collapsed) return false;
    const span = document.createElement('span');
    span.className = 'annotation-highlight note';
    span.dataset.noteId = id;
    try {
      range.surroundContents(span);
      return true;
    } catch {
      // The quote spans an element boundary; the panel still navigates by line.
      return false;
    }
  }

  function highlight(quote, id) {
    const map = textMap();
    const text = body.textContent;
    let startIndex = text.indexOf(quote);
    let length = quote.length;
    if (startIndex < 0) {
      const pattern = new RegExp(
        quote.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/\s+/g, '\\s+'),
        'i',
      );
      const match = pattern.exec(text);
      if (!match) return false;
      startIndex = match.index;
      length = match[0].length;
    }
    const start = locate(map.nodes, startIndex);
    const end = locate(map.nodes, startIndex + length);
    if (!start || !end) return false;
    return wrapRange(start, end, id);
  }

  function applyHighlights() {
    unwrapHighlights();
    for (const note of notes) {
      const quote = String(note.originalText ?? '').trim();
      if (quote.length < 2) continue;
      highlight(quote, note.id);
    }
  }

  // ── Selection toolbar ────────────────────────────────────────────────────

  function captureSelection() {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
    const range = selection.getRangeAt(0);
    if (!body.contains(range.commonAncestorContainer)) return null;
    const quote = selection.toString().replace(/\s+/g, ' ').trim();
    if (quote.length < 2) return null;
    const block = owningBlock(range.startContainer, body);
    return {
      quote,
      line: Number(block?.dataset?.line) || 0,
      start: block ? textOffsetWithin(block, range.startContainer, range.startOffset) : 0,
      end: block ? textOffsetWithin(block, range.endContainer, range.endOffset) : 0,
      rect: range.getBoundingClientRect(),
    };
  }

  function handleSelection() {
    const captured = captureSelection();
    if (!captured) {
      hideToolbar();
      return;
    }
    pending = captured;
    toolbar.hidden = false;
    const left = Math.min(
      Math.max(8, captured.rect.left + captured.rect.width / 2 - 26),
      window.innerWidth - 96,
    );
    toolbar.style.left = `${left}px`;
    toolbar.style.top = captured.rect.top < 64
      ? `${captured.rect.bottom + 10}px`
      : `${captured.rect.top - 44}px`;
  }

  function hideToolbar() {
    toolbar.hidden = true;
    pending = null;
  }

  // ── Popover ──────────────────────────────────────────────────────────────

  function positionPopover(rect) {
    const width = Math.min(360, window.innerWidth - 24);
    const left = Math.min(
      Math.max(12, (rect?.left ?? window.innerWidth / 2) - width / 2),
      window.innerWidth - width - 12,
    );
    const top = Math.min(Math.max(64, (rect?.bottom ?? 160) + 10), window.innerHeight - 220);
    popover.style.left = `${left}px`;
    popover.style.top = `${top}px`;
    popover.style.width = `${width}px`;
  }

  function openPopover({ title, quote, value, rect }) {
    popoverTitle.textContent = title;
    popoverQuote.textContent = quote ? `“${quote}”` : '';
    popoverQuote.hidden = !quote;
    textarea.value = value;
    popoverError.hidden = true;
    popover.hidden = false;
    positionPopover(rect);
    textarea.focus();
  }

  function beginNote() {
    if (!pending) return;
    editing = null;
    openPopover({
      title: 'New note',
      quote: pending.quote,
      value: '',
      rect: pending.rect,
    });
  }

  function beginEdit(id) {
    const note = notes.find((candidate) => candidate.id === id);
    if (!note) return;
    editing = id;
    const card = list.querySelector(`.note-card[data-note-id="${CSS.escape(id)}"]`);
    openPopover({
      title: 'Edit note',
      quote: note.originalText,
      value: note.text ?? '',
      rect: card?.getBoundingClientRect(),
    });
  }

  function closePopover() {
    popover.hidden = true;
    editing = null;
  }

  async function submit() {
    const text = textarea.value.trim();
    if (!text) {
      popoverError.textContent = 'Write something first.';
      popoverError.hidden = false;
      textarea.focus();
      return;
    }
    const existing = editing ? notes.find((note) => note.id === editing) : null;
    const note = existing
      ? { ...existing, text }
      : buildNote({
        id: newNoteId(),
        text,
        quote: pending?.quote ?? '',
        line: pending?.line ?? 1,
        start: pending?.start ?? 0,
        end: pending?.end ?? 0,
      });
    try {
      const response = await api.saveNote(rel, note);
      setNotes(response.notes);
      closePopover();
      hideToolbar();
      notify?.(existing ? 'Note updated' : 'Note saved');
    } catch (error) {
      popoverError.textContent = error.message;
      popoverError.hidden = false;
    }
  }

  async function removeNote(id) {
    if (!window.confirm('Delete this note?')) return;
    try {
      const response = await api.deleteNote(rel, id);
      setNotes(response.notes);
      notify?.('Note deleted');
    } catch (error) {
      notify?.(`Could not delete the note: ${error.message}`);
    }
  }

  // ── Navigation ───────────────────────────────────────────────────────────

  function flash(element, className) {
    if (!element) return;
    element.classList.remove(className);
    void element.offsetWidth;
    element.classList.add(className);
    element.addEventListener('animationend', () => element.classList.remove(className), { once: true });
  }

  function focusNote(id) {
    const note = notes.find((candidate) => candidate.id === id);
    const target = body.querySelector(`span.annotation-highlight[data-note-id="${CSS.escape(id)}"]`);
    if (target) {
      target.scrollIntoView({ block: 'center', behavior: 'smooth' });
      flash(target, 'highlight-flash');
    } else if (note) {
      const line = noteLine(note);
      const block = line > 0 ? body.querySelector(`[data-line="${line}"]`) : null;
      block?.scrollIntoView({ block: 'start' });
    }
    const card = list.querySelector(`.note-card[data-note-id="${CSS.escape(id)}"]`);
    card?.scrollIntoView({ block: 'nearest' });
    flash(card, 'note-flash');
  }

  body.addEventListener('click', (event) => {
    const span = event.target.closest?.('span.annotation-highlight');
    if (!span) return;
    const id = span.dataset.noteId;
    if (id) focusNote(id);
  });

  function setResource(value) {
    rel = value;
    notes = [];
    pending = null;
    editing = null;
    hideToolbar();
    closePopover();
    renderPanel();
  }

  function setNotes(value) {
    notes = sortNotes(value);
    renderPanel();
    applyHighlights();
  }

  return {
    setResource,
    setNotes,
    getNotes: () => notes,
    handleSelection,
    hideToolbar,
    beginNote,
    beginEdit,
    submit,
    closePopover,
    removeNote,
    focusNote,
  };
}
