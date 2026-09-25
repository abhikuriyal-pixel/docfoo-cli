/**
 * The card browser: breadcrumbs, cover cards and their fallbacks. Mirrors the
 * desktop resource browser (folders first, then files, alphabetical) without
 * any create/rename/delete/move affordances.
 */

import { cardMeta, kindLabel } from './covers.js';

export const DICE_ICON = `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="3"/><circle cx="8.2" cy="8.2" r="1.1" fill="currentColor"/><circle cx="15.8" cy="8.2" r="1.1" fill="currentColor"/><circle cx="12" cy="12" r="1.1" fill="currentColor"/><circle cx="8.2" cy="15.8" r="1.1" fill="currentColor"/><circle cx="15.8" cy="15.8" r="1.1" fill="currentColor"/></svg>`;

function kindGlyph(kind) {
  const common = 'viewBox="0 0 24 24" width="30" height="30" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"';
  if (kind === 'md') {
    return `<svg ${common}><path d="M6 3h9l4 4v14H6z"/><path d="M14 3v5h5"/><path d="M9 13h6M9 17h6"/></svg>`;
  }
  if (kind === 'image') {
    return `<svg ${common}><rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.4"/><path d="M4 17l5-5 4 4 3-3 4 4"/></svg>`;
  }
  if (kind === 'notebook') {
    return `<svg ${common}><rect x="5" y="3" width="14" height="18" rx="2"/><path d="M9 8h6M9 12h6M9 16h3"/></svg>`;
  }
  return `<svg ${common}><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v5h5"/></svg>`;
}

export function imageSrc(rel) {
  return `/api/asset?path=${encodeURIComponent(rel)}`;
}

const CHEVRON_LEFT = `<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="15 18 9 12 15 6"/></svg>`;
const CHEVRON_SEP = `<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="9 18 15 12 9 6"/></svg>`;
const HOME_ICON = `<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/></svg>`;

/**
 * Breadcrumbs: the desktop's chevron `icon-btn` back control, a home icon on
 * the root segment, then one segment per folder. `handlers` is
 * `{ onNavigate(path), onBack(), canGoBack }`.
 */
export function renderCrumbs(container, rel, handlers = {}) {
  const { onNavigate, onBack, canGoBack = false } = handlers;
  container.textContent = '';

  const back = document.createElement('button');
  back.type = 'button';
  back.className = 'icon-btn';
  back.title = 'Back';
  back.setAttribute('aria-label', 'Back');
  back.innerHTML = CHEVRON_LEFT;
  back.disabled = !canGoBack;
  if (canGoBack) back.addEventListener('click', () => onBack?.());
  container.appendChild(back);

  const segments = String(rel ?? '').split('/').filter(Boolean);
  const home = document.createElement('button');
  home.type = 'button';
  home.className = `breadcrumb-item home${segments.length === 0 ? ' current' : ''}`;
  home.innerHTML = `${HOME_ICON}<span>Resources</span>`;
  home.disabled = segments.length === 0;
  if (segments.length > 0) home.addEventListener('click', () => onNavigate?.(''));
  container.appendChild(home);

  let path = '';
  segments.forEach((segment, index) => {
    const separator = document.createElement('span');
    separator.className = 'breadcrumb-sep';
    separator.innerHTML = CHEVRON_SEP;
    container.appendChild(separator);

    path = path ? `${path}/${segment}` : segment;
    const last = index === segments.length - 1;
    const button = document.createElement('button');
    button.type = 'button';
    button.className = `breadcrumb-item${last ? ' current' : ''}`;
    button.textContent = segment;
    button.disabled = last;
    if (!last) button.addEventListener('click', () => onNavigate?.(path));
    container.appendChild(button);
  });
}

export function levelTitle(rel) {
  const segments = String(rel ?? '').split('/').filter(Boolean);
  return segments.length > 0 ? segments[segments.length - 1] : 'Resources';
}

export function levelDescription(entries) {
  const folders = entries.filter((entry) => entry.kind === 'dir').length;
  const docs = entries.filter((entry) => entry.kind === 'md').length;
  const parts = [];
  if (folders) parts.push(`${folders} folder${folders === 1 ? '' : 's'}`);
  if (docs) parts.push(`${docs} document${docs === 1 ? '' : 's'}`);
  return parts.length > 0 ? parts.join(' · ') : 'Empty folder';
}

/** One cover: random figure, collage, monogram or kind glyph. */
function coverElement(entry, plan) {
  const cover = document.createElement('div');
  const modifier = plan.shape === 'collage' ? ` n${plan.figures.length}` : '';
  cover.className = `res-card-cover res-card-cover--${plan.shape}${modifier}`;
  if (plan.shape === 'figure' || plan.shape === 'collage') {
    for (const figure of plan.figures) {
      const img = document.createElement('img');
      img.loading = 'lazy';
      img.decoding = 'async';
      img.alt = '';
      img.addEventListener('load', () => img.classList.add('loaded'), { once: true });
      img.src = imageSrc(figure);
      cover.appendChild(img);
    }
  } else if (plan.shape === 'mark') {
    const monogram = document.createElement('span');
    monogram.className = 'res-card-monogram';
    monogram.textContent = 'CM';
    cover.appendChild(monogram);
  } else {
    cover.innerHTML = kindGlyph(plan.kind);
  }
  return cover;
}

/** The whole grid. `handlers.onOpen(entry)` receives the clicked card. */
export function renderGrid(container, entries, picker, handlers) {
  container.textContent = '';
  for (const entry of entries) {
    const card = document.createElement('article');
    card.className = `res-card res-card--${entry.kind}`;
    card.tabIndex = 0;
    card.dataset.rel = entry.rel;
    card.appendChild(coverElement(entry, picker.plan(entry)));

    const body = document.createElement('div');
    body.className = 'res-card-body';
    const name = document.createElement('h3');
    name.className = 'res-card-name';
    name.textContent = entry.name;
    name.title = entry.rel;
    const meta = document.createElement('p');
    meta.className = 'res-card-meta';
    meta.textContent = cardMeta(entry);
    body.append(name, meta);

    const kind = document.createElement('span');
    kind.className = 'res-card-kind';
    kind.textContent = kindLabel(entry.kind);
    card.append(body, kind);

    const activate = () => handlers.onOpen(entry);
    card.addEventListener('click', activate);
    card.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        activate();
      }
    });
    container.appendChild(card);
  }
}
