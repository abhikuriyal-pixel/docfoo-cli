/* Runs before paint. A private site key never changes PiKins' preferences. */
(() => {
  document.documentElement.classList.add('js');
  let saved;
  try { saved = localStorage.getItem('docfoo-theme'); } catch { /* Storage may be unavailable. */ }
  const theme = saved === 'light' || saved === 'dark' ? saved : (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  document.documentElement.dataset.theme = theme;
})();
