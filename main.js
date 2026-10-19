'use strict';

(() => {
  const $ = (selector, parent = document) => parent.querySelector(selector);
  const $$ = (selector, parent = document) => [...parent.querySelectorAll(selector)];
  const root = document.documentElement;
  const preference = matchMedia('(prefers-color-scheme: dark)');
  const themes = $('.theme-control');

  function storedTheme() {
    try { return localStorage.getItem('docfoo-theme'); } catch { return null; }
  }
  function setTheme(theme) {
    root.dataset.theme = theme;
    $$('button[data-theme]', themes).forEach(button => button.setAttribute('aria-pressed', String(button.dataset.theme === theme)));
    $('meta[name="theme-color"]').content = theme === 'dark' ? '#080a09' : '#dadfdb';
  }
  themes.hidden = false;
  setTheme(root.dataset.theme);
  $$('button[data-theme]', themes).forEach(button => button.addEventListener('click', () => {
    setTheme(button.dataset.theme);
    try { localStorage.setItem('docfoo-theme', button.dataset.theme); } catch { /* This visit still updates. */ }
  }));
  preference.addEventListener('change', event => {
    if (!['light', 'dark'].includes(storedTheme())) setTheme(event.matches ? 'dark' : 'light');
  });
  window.addEventListener('storage', event => {
    if (event.key === 'docfoo-theme') setTheme(['light', 'dark'].includes(event.newValue) ? event.newValue : preference.matches ? 'dark' : 'light');
  });

  const nav = $('#site-nav');
  const menu = $('.menu-toggle');
  function closeMenu(focus = false) {
    nav.classList.remove('is-open');
    menu.setAttribute('aria-expanded', 'false');
    menu.setAttribute('aria-label', 'Open navigation');
    if (focus) menu.focus();
  }
  menu.hidden = false;
  menu.addEventListener('click', () => {
    const open = menu.getAttribute('aria-expanded') !== 'true';
    nav.classList.toggle('is-open', open);
    menu.setAttribute('aria-expanded', String(open));
    menu.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation');
  });
  $$('a', nav).forEach(link => link.addEventListener('click', () => closeMenu()));
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && nav.classList.contains('is-open')) { closeMenu(true); event.preventDefault(); }
  });
  document.addEventListener('click', event => {
    if (!event.target.closest('.site-header')) closeMenu();
  });
  matchMedia('(min-width: 801px)').addEventListener('change', event => { if (event.matches) closeMenu(); });

  // Decorative fog pauses when off-screen or when the tab is hidden. CSS
  // independently disables it for the user's reduced-motion preference.
  const heroArt = $('.hero-art');
  if (heroArt) {
    let inView = !('IntersectionObserver' in window);
    const updateFog = () => heroArt.toggleAttribute('data-fog-active', inView && !document.hidden);
    if ('IntersectionObserver' in window) {
      const observer = new IntersectionObserver(entries => {
        inView = entries[0].isIntersecting;
        updateFog();
      }, { rootMargin: '24px' });
      observer.observe(heroArt);
    }
    document.addEventListener('visibilitychange', updateFog);
    updateFog();
  }

  // Architecture is a complete static diagram first. Motion runs once, only
  // while visible, and never advances under a reduced-motion preference.
  const architecture = $('.architecture-graphic');
  if (architecture) {
    const steps = $$('[data-architecture-step]', architecture);
    const progress = $$('.architecture-progress span', architecture);
    const controls = $('.architecture-controls', architecture);
    const playback = $('.architecture-playback', architecture);
    const toggle = $('[data-architecture-toggle]', architecture);
    const replay = $('[data-architecture-replay]', architecture);
    const reduced = matchMedia('(prefers-reduced-motion: reduce)');
    const durations = [1800, 2400, 2400, 2000, 3000, 2800, 2800];
    let index = 0;
    let requested = true;
    let finished = false;
    let inView = !('IntersectionObserver' in window);
    let remaining = durations[0];
    let timer = null;
    let started = 0;

    function stopClock() {
      if (timer === null) return;
      clearTimeout(timer);
      timer = null;
      remaining = Math.max(0, remaining - (performance.now() - started));
    }
    function showStep() {
      steps.forEach((step, position) => {
        step.classList.toggle('is-current', !reduced.matches && position === index);
        step.classList.toggle('is-complete', !reduced.matches && (finished || position < index));
      });
      progress.forEach((segment, position) => {
        segment.classList.toggle('is-current', position === index);
        segment.classList.toggle('is-complete', finished || position < index);
      });
      $('.architecture-step-label', architecture).textContent = `${String(index + 1).padStart(2, '0')} / 07 · ${$('h4', steps[index]).textContent}`;
      $('#architecture-caption').textContent = steps[index].dataset.caption;
    }
    function syncMotion() {
      const motion = !reduced.matches;
      controls.hidden = !motion;
      playback.hidden = !motion;
      architecture.toggleAttribute('data-motion-ready', motion);
      const running = motion && requested && !finished && inView && !document.hidden;
      if (!running) stopClock();
      architecture.dataset.architectureState = !motion ? 'static' : finished ? 'complete' : running ? 'playing' : 'paused';
      toggle.textContent = requested && !finished ? 'Pause' : 'Play';
      toggle.setAttribute('aria-label', `${requested && !finished ? 'Pause' : 'Play'} architecture animation`);
      if (running && timer === null) {
        started = performance.now();
        timer = setTimeout(() => {
          timer = null;
          if (index === steps.length - 1) {
            finished = true;
            requested = false;
          } else {
            index += 1;
            remaining = durations[index];
          }
          showStep();
          syncMotion();
        }, Math.max(20, remaining));
      }
    }
    function restart() {
      stopClock();
      index = 0;
      remaining = durations[0];
      finished = false;
      requested = true;
      showStep();
      syncMotion();
    }
    toggle.addEventListener('click', () => {
      if (finished) { restart(); return; }
      requested = !requested;
      syncMotion();
    });
    replay.addEventListener('click', restart);
    reduced.addEventListener('change', () => {
      stopClock();
      showStep();
      syncMotion();
    });
    document.addEventListener('visibilitychange', syncMotion);
    if ('IntersectionObserver' in window) {
      const observer = new IntersectionObserver(entries => {
        inView = entries[0].isIntersecting;
        syncMotion();
      }, { threshold: .08 });
      observer.observe(architecture);
    }
    showStep();
    syncMotion();
  }

  // Only the recorded-query area is tabbed. Product sections remain normal sections.
  const explorer = $('.query-explorer');
  const queryTabs = $$('.query-tabs [data-query]');
  const queryPanels = $$('[data-query-panel]');
  function selectQuery(key, focus = false) {
    const chosen = queryTabs.find(button => button.dataset.query === key);
    if (!chosen) return;
    queryTabs.forEach(button => {
      const selected = button === chosen;
      button.setAttribute('aria-selected', String(selected));
      button.tabIndex = selected ? 0 : -1;
    });
    queryPanels.forEach(panel => {
      const selected = panel.dataset.queryPanel === key;
      panel.hidden = !selected;
      panel.setAttribute('role', 'tabpanel');
      panel.setAttribute('aria-labelledby', 'query-tab-' + panel.dataset.queryPanel);
      panel.tabIndex = 0;
    });
    explorer.dataset.activeQuery = key;
    if (focus) chosen.focus({ preventScroll: true });
  }
  if (queryTabs.length) {
    explorer.classList.add('is-enhanced');
    $('.query-tabs').hidden = false;
    queryTabs.forEach((button, index) => {
      button.addEventListener('click', () => selectQuery(button.dataset.query));
      button.addEventListener('keydown', event => {
        let next;
        if (event.key === 'ArrowRight') next = (index + 1) % queryTabs.length;
        if (event.key === 'ArrowLeft') next = (index - 1 + queryTabs.length) % queryTabs.length;
        if (event.key === 'Home') next = 0;
        if (event.key === 'End') next = queryTabs.length - 1;
        if (next !== undefined) {
          event.preventDefault();
          selectQuery(queryTabs[next].dataset.query, true);
        }
      });
    });
    const linked = location.hash.startsWith('#query-') ? location.hash.slice(7) : '';
    selectQuery(queryTabs.some(button => button.dataset.query === linked) ? linked : queryTabs[0].dataset.query);
    $$('[data-query-target]').forEach(link => link.addEventListener('click', event => {
      event.preventDefault();
      selectQuery(link.dataset.queryTarget, true);
      $('#example').scrollIntoView({ block: 'start', behavior: 'instant' });
    }));
  }

  const timers = new WeakMap();
  $$('[data-copy]').forEach(button => {
    button.hidden = false;
    const original = button.textContent;
    button.addEventListener('click', async () => {
      const target = document.getElementById(button.dataset.copy);
      if (!target) return;
      clearTimeout(timers.get(button));
      try {
        if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
        await navigator.clipboard.writeText(target.textContent.trim());
        button.textContent = 'Copied';
        $('#copy-status').textContent = 'Copied to clipboard.';
      } catch {
        const range = document.createRange();
        range.selectNodeContents(target);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        button.textContent = 'Text selected';
        $('#copy-status').textContent = 'Clipboard unavailable. Text selected; use your device’s copy command.';
      }
      timers.set(button, setTimeout(() => { button.textContent = original; }, 2200));
    });
  });

  $$('[data-close-dialog]').forEach(button => button.addEventListener('click', () => button.closest('dialog').close()));
  $$('dialog').forEach(dialog => dialog.addEventListener('click', event => {
    if (event.target !== dialog) return;
    const rect = dialog.getBoundingClientRect();
    if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) dialog.close();
  }));

  const sourceDialog = $('#source-dialog');
  if (typeof sourceDialog.showModal !== 'function') return;

  // Source text is inserted only as text nodes, never interpreted as HTML.
  if (window.DOCFOO_EVIDENCE?.sources) {
    $$('[data-source]').forEach(link => {
      link.setAttribute('aria-haspopup', 'dialog');
      link.addEventListener('click', event => {
        const source = window.DOCFOO_EVIDENCE.sources[link.dataset.source];
        if (!source) return;
        event.preventDefault();
        $('#source-range').textContent = `Original stored source · lines ${source.start}–${source.end}`;
        $('#source-title-dialog').textContent = source.file;
        const fragment = document.createDocumentFragment();
        source.lines.forEach((text, index) => {
          const line = document.createElement('div');
          line.className = 'source-line';
          const number = document.createElement('span');
          number.className = 'source-number';
          number.setAttribute('aria-hidden', 'true');
          number.textContent = source.start + index;
          const code = document.createElement('code');
          code.textContent = text || ' ';
          line.append(number, code);
          fragment.append(line);
        });
        $('#source-lines').replaceChildren(fragment);
        $('#source-download').href = source.download;
        sourceDialog.showModal();
        $('#source-lines').scrollTop = 0;
      });
    });
  }

  const imageDialog = $('#image-dialog');
  $$('[data-enlarge]').forEach(link => {
    link.setAttribute('aria-haspopup', 'dialog');
    link.addEventListener('click', event => {
      event.preventDefault();
      const image = $('#enlarged-image');
      const original = link.querySelector('img') || link.closest('figure')?.querySelector('img');
      image.src = link.href;
      image.alt = link.dataset.imageAlt || original?.alt || 'Original source image';
      const width = Number(link.dataset.imageWidth) || original?.naturalWidth;
      const height = Number(link.dataset.imageHeight) || original?.naturalHeight;
      if (width && height) { image.width = width; image.height = height; }
      else { image.removeAttribute('width'); image.removeAttribute('height'); }
      $('#image-original').href = link.href;
      $('#image-title').textContent = link.dataset.imageTitle || link.closest('figure')?.querySelector('figcaption strong')?.textContent || 'Original source image';
      imageDialog.showModal();
    });
  });
})();
