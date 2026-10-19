'use strict';

// A finite 1x replay of the selected capture, NOT a live model call. Endpoints
// and file-tool events are recorded; first-text offsets are reconstructed and
// the text reveal between them is interpolated (there are no saved chunks).
(() => {
  const section = document.querySelector('#search-comparison');
  const data = window.DOCFOO_EVIDENCE?.comparison;
  if (!section || data?.schema !== 'docfoo.comparison-replay/1') return;
  const $ = selector => section.querySelector(selector);
  const $$ = selector => [...section.querySelectorAll(selector)];
  const methods = ['docfoo', 'agentic'];
  if (!methods.every(key => Number.isFinite(data[key]?.first_text_seconds_approx)
      && data[key].first_text_seconds_approx >= 0
      && data[key].end_seconds > data[key].first_text_seconds_approx)
      || !Array.isArray(data.tools)) return;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const printing = matchMedia('print');
  const controls = $('.comparison-controls');
  const toggle = $('[data-comparison-toggle]');
  const total = Math.max(...methods.map(key => data[key].end_seconds));
  const panels = {};

  // Keep the server-rendered Markdown/MathML DOM and source-link listeners.
  // Only text-node contents are clipped; equations and citations reveal as
  // complete semantic units, never as broken math/HTML markup.
  for (const key of methods) {
    const panel = $(`[data-comparison-method="${key}"]`);
    const answer = panel.querySelector('.comparison-answer');
    let length = 0;
    const parts = [];
    const blocks = [];
    function visit(node) {
      if (node.nodeType === Node.TEXT_NODE) {
        const text = node.nodeValue;
        parts.push({ node, text, start: length, end: length + text.length });
        length += text.length;
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        if (node.matches('.katex, math, a')) {
          const size = node.textContent.length;
          parts.push({ node, start: length, end: length + size });
          length += size;
        } else {
          for (const child of node.childNodes) visit(child);
        }
      }
    }
    for (const child of answer.childNodes) {
      const start = length;
      visit(child);
      if (child.nodeType === Node.ELEMENT_NODE) blocks.push({ node: child, start });
    }
    panels[key] = { panel, answer, parts, blocks, length,
      state: panel.querySelector('.comparison-state'), time: panel.querySelector('.comparison-time'),
      activity: panel.querySelector('.comparison-activity') };
    panel.querySelector('.comparison-progress').hidden = false;
  }
  const activity = data.tools.map(tool => {
    const li = document.createElement('li');
    const name = document.createElement('code');
    name.textContent = tool.name;
    const description = document.createElement('span');
    const path = tool.arguments?.path;
    description.textContent = path && path !== '.' ? path.replace('/content.md', '') : 'Papers scans';
    const time = document.createElement('small');
    li.append(name, description, time);
    panels.agentic.activity.append(li);
    return { ...tool, li, time };
  });
  // Complete answers are the initial view, including on reload. A replay may
  // temporarily clear them only after the visitor explicitly starts it.
  let elapsed = total;
  let epoch = null;
  let frame = null;
  let requested = false;
  let finished = true;
  let hasPlayed = false;
  let inView = !('IntersectionObserver' in window);
  let lastPaint = -Infinity;
  const staticView = () => reduced.matches || printing.matches;
  const nowSeconds = () => Math.min(total, elapsed + (epoch === null ? 0 : (performance.now() - epoch) / 1000));
  function stopClock() {
    elapsed = nowSeconds();
    epoch = null;
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
  }
  function setText(node, text) { if (node.textContent !== text) node.textContent = text; }
  function render(seconds, all = false) {
    setText($('.comparison-clock'), `${seconds.toFixed(1)} s`);
    for (const key of methods) {
      const p = panels[key];
      const timing = data[key];
      const done = all || seconds >= timing.end_seconds;
      const writing = seconds >= timing.first_text_seconds_approx;
      const fraction = done ? 1 : Math.max(0, (seconds - timing.first_text_seconds_approx) / (timing.end_seconds - timing.first_text_seconds_approx));
      const count = done ? p.length : Math.floor(p.length * fraction);
      for (const part of p.parts) {
        if (part.text !== undefined) {
          const next = part.text.slice(0, Math.max(0, Math.min(part.text.length, count - part.start)));
          if (part.node.nodeValue !== next) part.node.nodeValue = next;
        } else part.node.hidden = !done && count <= part.start;
      }
      for (const block of p.blocks) block.node.hidden = !done && count <= block.start;
      p.answer.setAttribute('aria-busy', String(!done));
      p.panel.dataset.phase = done ? 'done' : writing ? 'writing' : 'waiting';
      let label = done ? 'Answer complete' : writing ? 'Writing answer' : key === 'docfoo' ? 'Preparing evidence / waiting for writer' : 'Model selecting search steps';
      if (key === 'agentic') {
        const active = activity.filter(tool => seconds >= tool.started_seconds && seconds < tool.started_seconds + tool.seconds);
        if (!writing && !done) {
          if (active.length) label = 'Using file tools: ' + [...new Set(active.map(tool => tool.name))].join(' + ');
          else if (seconds > Math.max(...activity.map(tool => tool.started_seconds + tool.seconds))) label = 'Preparing final answer';
        }
        p.activity.hidden = writing || done;
        for (const tool of activity) {
          tool.li.hidden = seconds < tool.started_seconds;
          const running = seconds < tool.started_seconds + tool.seconds;
          tool.li.toggleAttribute('data-running', running);
          setText(tool.time, running ? 'running' : `${tool.seconds.toFixed(2)} s`);
        }
      }
      setText(p.state, label);
      setText(p.time, `${Math.min(seconds, timing.end_seconds).toFixed(2)} s`);
    }
  }
  function updateControls() {
    controls.hidden = staticView();
    toggle.hidden = finished;
    $('[data-comparison-complete]').hidden = finished;
    $('.comparison-clock').hidden = finished;
    setText($('[data-comparison-replay]'), hasPlayed ? 'Replay' : 'Play replay');
    section.dataset.comparisonState = staticView() ? 'static' : finished ? 'complete' : epoch !== null ? 'playing' : 'paused';
    setText(toggle, finished ? 'Play' : requested ? 'Pause' : 'Play');
    toggle.setAttribute('aria-label', finished ? 'Play comparison replay' : requested ? 'Pause comparison replay' : 'Play comparison replay');
  }
  function tick(timestamp) {
    frame = null;
    const seconds = nowSeconds();
    if (seconds >= total) {
      elapsed = total;
      epoch = null;
      finished = true;
      requested = false;
      render(total, true);
      updateControls();
      setText($('#comparison-replay-status'), `Replay complete. DocFoo: ${data.docfoo.end_seconds.toFixed(2)} seconds. Agentic search: ${data.agentic.end_seconds.toFixed(2)} seconds.`);
      return;
    }
    if (timestamp - lastPaint >= 40) { render(seconds); lastPaint = timestamp; }
    frame = requestAnimationFrame(tick);
  }
  function sync() {
    const running = !staticView() && requested && !finished && inView && !document.hidden;
    if (!running) stopClock();
    if (staticView()) {
      finished = true;
      requested = false;
      render(total, true);
    } else if (running && epoch === null) {
      epoch = performance.now();
      lastPaint = -Infinity;
      frame = requestAnimationFrame(tick);
    }
    updateControls();
  }
  function restart() {
    stopClock();
    elapsed = 0;
    finished = false;
    requested = true;
    hasPlayed = true;
    panels.docfoo.panel.querySelector('.comparison-reading').scrollTop = 0;
    panels.agentic.panel.querySelector('.comparison-reading').scrollTop = 0;
    setText($('#comparison-replay-status'), 'Recorded comparison replay started. Text reveal is interpolated.');
    render(0);
    sync();
  }
  function complete() {
    stopClock();
    elapsed = total;
    finished = true;
    requested = false;
    render(total, true);
    sync();
    setText($('#comparison-replay-status'), 'Complete recorded answers shown.');
  }
  toggle.addEventListener('click', () => {
    if (finished) restart();
    else { requested = !requested; sync(); }
  });
  $('[data-comparison-replay]').addEventListener('click', restart);
  $('[data-comparison-complete]').addEventListener('click', complete);
  document.addEventListener('visibilitychange', sync);
  reduced.addEventListener('change', sync);
  printing.addEventListener('change', sync);
  // Pause when a reader follows/focuses a source link rather than moving the
  // link out from under their keyboard focus while the answer is revealed.
  $$('.comparison-reading').forEach(reading => reading.addEventListener('focusin', () => {
    if (!finished) { requested = false; sync(); }
  }));
  if ('IntersectionObserver' in window) {
    const observer = new IntersectionObserver(entries => {
      inView = entries[0].isIntersecting;
      sync();
    }, { threshold: .05 });
    observer.observe($('.comparison-grid'));
  }
  render(total, true);
  sync();
})();
