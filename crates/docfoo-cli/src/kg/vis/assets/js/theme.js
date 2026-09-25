/** Theme names, persistence and application. */

export const THEME_STORAGE_KEY = 'docfoo-kg-vis-theme';
export const THEMES = ['kinetic', 'art-deco'];

export function normalizeTheme(value) {
  return THEMES.includes(value) ? value : 'art-deco';
}

export function readTheme(storage) {
  try {
    return normalizeTheme(storage && storage.getItem(THEME_STORAGE_KEY));
  } catch {
    return 'art-deco';
  }
}

export function writeTheme(storage, theme) {
  const value = normalizeTheme(theme);
  try {
    if (storage) storage.setItem(THEME_STORAGE_KEY, value);
  } catch {
    // Private browsing / storage disabled: the session still applies the value.
  }
  return value;
}

export function toggleTheme(theme) {
  return normalizeTheme(theme) === 'kinetic' ? 'art-deco' : 'kinetic';
}

/** Apply a theme to the document body (no-op outside the browser). */
export function applyTheme(theme, body = typeof document !== 'undefined' ? document.body : null) {
  const value = normalizeTheme(theme);
  if (body) body.dataset.theme = value;
  return value;
}
