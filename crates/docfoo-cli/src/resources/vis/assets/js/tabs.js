/**
 * Document tabs for the resource viewer — one tab per resource, browser-like
 * close behaviour and per-origin persistence (reload-safe; a fixed
 * `--port` is needed for tabs to survive a server restart).
 */

export const TABS_STORAGE_KEY = 'docfoo-res-vis-tabs';

export function addTab(tabs, doc) {
  const list = [...(tabs ?? [])];
  const index = list.findIndex((tab) => tab.rel === doc.rel);
  if (index >= 0) {
    list[index] = { ...list[index], ...doc };
    return list;
  }
  list.push({ rel: doc.rel, name: doc.name || doc.rel });
  return list;
}

export function removeTab(tabs, rel) {
  return (tabs ?? []).filter((tab) => tab.rel !== rel);
}

/** The tab to activate after closing `rel`: the next one, else the previous. */
export function neighborRel(tabs, rel) {
  const list = tabs ?? [];
  const index = list.findIndex((tab) => tab.rel === rel);
  if (index < 0) return null;
  const next = list[index + 1] ?? list[index - 1];
  return next ? next.rel : null;
}

export function saveTabs(storage, tabs, active) {
  try {
    storage?.setItem(TABS_STORAGE_KEY, JSON.stringify({ tabs, active }));
  } catch {
    // Private browsing / storage disabled: tabs still work for this page.
  }
}

export function loadTabs(storage) {
  try {
    const raw = storage?.getItem(TABS_STORAGE_KEY);
    if (!raw) return { tabs: [], active: null };
    const parsed = JSON.parse(raw);
    const tabs = Array.isArray(parsed?.tabs)
      ? parsed.tabs.filter((tab) => tab && typeof tab.rel === 'string')
      : [];
    const active = tabs.some((tab) => tab.rel === parsed?.active) ? parsed.active : null;
    return { tabs, active };
  } catch {
    return { tabs: [], active: null };
  }
}

/** Label a tab with its folder when two open documents share a name. */
export function tabLabels(tabs) {
  const counts = new Map();
  for (const tab of tabs ?? []) {
    const name = tab.name || tab.rel;
    counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return (tabs ?? []).map((tab) => {
    const name = tab.name || tab.rel;
    if ((counts.get(name) ?? 0) <= 1) return name;
    const parts = String(tab.rel).split('/');
    return parts.length > 1 ? `${parts[parts.length - 2]}/${name}` : name;
  });
}
