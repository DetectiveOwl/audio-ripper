(function () {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const $ = (id) => document.getElementById(id);
  const rack = $('rack'), urlEl = $('url'), go = $('go'), fill = $('fill'),
        pct = $('pct'), msg = $('msg'), readout = $('readout'),
        outdirEl = $('outdir'), pick = $('pick'), fmtWrap = document.querySelector('.fmt'),
        nameEl = $('name-style'), cookiesEl = $('cookies'),
        qlist = $('qlist'), qcount = $('qcount'), qclear = $('qclear'),
        toolsEl = $('tools'), checkBtn = $('check');
  const fmtBtns = [...document.querySelectorAll('.fmt button')];

  let fmt = 'mp3';
  // Queue: links wait here and are extracted one at a time.
  // item = { id, url, fmt, state: waiting|running|done|error|cancelled,
  //          title, progress, message, detail, file, files }
  let items = [];
  let nextId = 1;
  let current = null;     // the running item
  let paused = false;     // set by Cancel: the queue stops until the user starts it again
  let toolsBusy = false;  // yt-dlp / spotDL are being updated
  let lastCookies = 'none';

  if (!/Mac/i.test(navigator.userAgent)) $('opt-safari').remove();

  function setSelected(b) {
    fmt = b.dataset.fmt;
    fmtBtns.forEach((x) => {
      x.classList.toggle('on', x === b);
      x.setAttribute('aria-checked', x === b ? 'true' : 'false');
    });
  }
  fmtBtns.forEach((b) => b.addEventListener('click', () => {
    setSelected(b);
    invoke('set_option', { key: 'format', value: fmt }).catch(() => {});
  }));

  nameEl.addEventListener('change', () => {
    invoke('set_option', { key: 'name_style', value: nameEl.value }).catch(() => {});
  });
  cookiesEl.addEventListener('change', async () => {
    const value = cookiesEl.value;
    try {
      if (value === 'file') {
        const imported = await invoke('import_cookies');
        if (!imported) { cookiesEl.value = lastCookies; return; }
      } else {
        await invoke('set_option', { key: 'cookies', value });
      }
      lastCookies = value;
    } catch (e) {
      cookiesEl.value = lastCookies;
      setReadout('ERROR: ' + e, 'err');
    }
  });

  function setReadout(text, cls, detail) {
    readout.textContent = text;
    readout.className = 'readout' + (cls ? ' ' + cls : '');
    readout.title = detail || '';
  }
  function setProgress(p, m) {
    fill.style.width = p + '%';
    pct.textContent = p + '%';
    if (m) msg.textContent = m;
  }
  function setRunning(on) {
    rack.classList.toggle('busy', on);
    go.textContent = on ? 'Cancel' : 'Extract audio';
    go.classList.toggle('stop', on);
    pick.disabled = on;
    fmtWrap.classList.toggle('locked', on);
  }
  function showOutDir(dir) {
    outdirEl.textContent = dir;
    outdirEl.title = dir;
  }

  // ---- queue -------------------------------------------------------------

  const waiting = () => items.filter((i) => i.state === 'waiting');

  function shortUrl(u) {
    try {
      const x = new URL(u);
      return x.hostname.replace(/^www\./, '') + x.pathname + x.search;
    } catch (_) { return u; }
  }

  let renderQueued = false;
  function scheduleRender() {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => { renderQueued = false; renderQueue(); });
  }

  function statusText(i) {
    switch (i.state) {
      case 'waiting': return 'waiting';
      case 'running': return i.progress + '%';
      case 'done': return i.files > 1 ? i.files + ' tracks' : 'done';
      case 'error': return 'error';
      default: return 'cancelled';
    }
  }

  function renderQueue() {
    const n = waiting().length;
    qcount.textContent = n ? 'Queue (' + n + ' waiting)' : 'Queue';
    qclear.hidden = !items.some((i) => i.state !== 'waiting' && i.state !== 'running');
    qlist.textContent = '';
    if (!items.length) {
      const li = document.createElement('li');
      li.className = 'empty';
      li.textContent = 'Paste one or more links above: they are extracted here one at a time.';
      qlist.append(li);
    }
    for (const i of items) {
      const li = document.createElement('li');
      li.className = i.state;
      const dot = document.createElement('span'); dot.className = 'dot';
      const t = document.createElement('span'); t.className = 'qt';
      t.textContent = i.state === 'error' ? i.message : (i.title || shortUrl(i.url));
      t.title = i.state === 'error' ? (i.detail || i.message) : i.url;
      const s = document.createElement('span'); s.className = 'qs'; s.textContent = statusText(i);
      li.append(dot, t, s);
      if (i.state === 'waiting' || i.state === 'error' || i.state === 'cancelled') {
        const x = document.createElement('button');
        x.type = 'button'; x.className = 'qx'; x.textContent = '✕';
        x.title = 'Remove';
        x.addEventListener('click', () => { items = items.filter((o) => o !== i); scheduleRender(); updateIdleUi(); });
        li.append(x);
      } else if (i.state === 'done' && i.file) {
        const x = document.createElement('button');
        x.type = 'button'; x.className = 'qx'; x.textContent = 'show';
        x.title = 'Show in folder';
        x.addEventListener('click', () => invoke('reveal', { path: i.file }).catch(() => {}));
        li.append(x);
      }
      qlist.append(li);
    }
    const running = qlist.querySelector('.running');
    if (running) running.scrollIntoView({ block: 'nearest' });
  }

  qclear.addEventListener('click', () => {
    items = items.filter((i) => i.state === 'waiting' || i.state === 'running');
    scheduleRender();
  });

  // What the idle display says: how many links wait, and why nothing starts.
  function updateIdleUi() {
    if (current) return;
    const n = waiting().length;
    if (n && toolsBusy) setProgressMsg('Waiting for the components update, then ' + n + ' queued.');
    else if (n && paused) setProgressMsg('Paused: ' + n + ' queued. Press Extract audio to continue.');
    go.textContent = 'Extract audio';
  }
  function setProgressMsg(m) { msg.textContent = m; }

  function addUrls(text) {
    const urls = text.split(/[\s,]+/).filter(Boolean);
    let added = 0;
    for (const url of urls) {
      if (items.some((i) => i.state === 'waiting' && i.url === url)) continue;
      items.push({ id: nextId++, url, fmt, state: 'waiting', title: null, progress: 0,
                   message: '', detail: null, file: null, files: 0 });
      added++;
    }
    scheduleRender();
    return added;
  }

  function pump() {
    if (current || paused || toolsBusy) { updateIdleUi(); return; }
    const next = waiting()[0];
    if (!next) { setRunning(false); updateIdleUi(); return; }
    current = next;
    next.state = 'running';
    setRunning(true);
    setProgress(0, 'Starting…');
    setReadout('CONNECTING…');
    scheduleRender();
    invoke('start_download', { url: next.url, fmt: next.fmt }).catch((e) => {
      const text = String(e);
      current = null;
      if (/being updated/i.test(text)) {   // an update started a moment ago: wait for it
        next.state = 'waiting';
        toolsBusy = true;
        setRunning(false);
        scheduleRender();
        updateIdleUi();
        return;
      }
      next.state = 'error';
      next.message = text;
      setReadout('ERROR: ' + text, 'err');
      scheduleRender();
      pump();
    });
  }

  // ---- progress from the backend -------------------------------------------

  function apply(s) {
    const it = current;
    if (!it) return;
    const queued = waiting().length;
    const more = queued ? '  (+' + queued + ' queued)' : '';
    if (s.status === 'running') {
      it.progress = s.progress;
      if (s.title) it.title = s.title;
      setProgress(s.progress, s.message + more);
      if (s.title) setReadout('▸ ' + s.title.toUpperCase());
    } else if (s.status === 'done') {
      it.state = 'done';
      it.progress = 100;
      it.title = s.title || it.title;
      it.file = s.file;
      it.files = s.files;
      current = null;
      setProgress(100, (s.message || 'Done.') + more);
      setReadout(s.title ? '✓ ' + s.title.toUpperCase() : '✓ DONE', 'ok');
      setRunning(false);
      pump();
    } else if (s.status === 'error') {
      it.state = 'error';
      it.message = s.message || 'unknown error.';
      it.detail = s.detail;
      current = null;
      setRunning(false);
      setProgress(0, 'Error.' + more);
      setReadout('ERROR: ' + it.message, 'err', s.detail);
      pump();
    } else if (s.status === 'cancelled') {
      it.state = 'cancelled';
      current = null;
      paused = true;
      setRunning(false);
      setProgress(0, 'Cancelled.');
      setReadout('CANCELLED.');
      updateIdleUi();
    }
    scheduleRender();
  }

  go.addEventListener('click', () => {
    if (current) { paused = true; invoke('cancel_download'); return; }
    const added = addUrls(urlEl.value);
    if (!added && !waiting().length) { setReadout('ERROR: ENTER A URL.', 'err'); return; }
    urlEl.value = '';
    paused = false;
    pump();
  });

  pick.addEventListener('click', async () => {
    try {
      const dir = await invoke('pick_out_dir');
      if (dir) showOutDir(dir);
    } catch (e) {
      setReadout('ERROR: ' + e, 'err');
    }
  });

  urlEl.addEventListener('keydown', (e) => { if (e.key === 'Enter') go.click(); });

  // ---- yt-dlp / spotDL updates ------------------------------------------------

  function showTools(t) {
    toolsBusy = t.state === 'checking';
    checkBtn.disabled = toolsBusy;
    let text;
    if (toolsBusy) {
      text = t.message || 'Checking for updates…';
    } else if (t.errors && t.errors.length) {
      text = 'Update check failed: ' + t.errors[0];
    } else if (t.updated && t.updated.length) {
      text = 'Updated: ' + t.updated.join(', ');
    } else if (t.ytdlp || t.spotdl) {
      text = 'yt-dlp ' + (t.ytdlp || '?') + ' · spotDL ' + (t.spotdl || '?');
    } else {
      text = 'Components: bundled versions.';
    }
    toolsEl.textContent = text;
    toolsEl.title = [].concat(t.errors || [], t.updated || []).join('\n');
    if (!toolsBusy) pump();       // downloads that waited for the update start now
    else updateIdleUi();
  }
  checkBtn.addEventListener('click', () => invoke('check_updates').catch(() => {}));
  listen('tools-status', (e) => showTools(e.payload));

  // Frameless window: own buttons, and the panel can be dragged to move it.
  $('win-min').addEventListener('click', () => invoke('minimize'));
  $('win-close').addEventListener('click', () => invoke('quit'));
  const appWindow = window.__TAURI__.window.getCurrentWindow();
  rack.addEventListener('mousedown', (e) => {
    if (e.button !== 0 || e.target.closest('button, input, a, select, label, li')) return;
    appWindow.startDragging();
  });

  listen('job-update', (e) => apply(e.payload));
  invoke('get_out_dir').then(showOutDir).catch(() => {});
  invoke('get_settings').then((s) => {
    const b = fmtBtns.find((x) => x.dataset.fmt === s.format);
    if (b) setSelected(b);
    nameEl.value = s.name_style;
    if ([...cookiesEl.options].some((o) => o.value === s.cookies)) cookiesEl.value = s.cookies;
    lastCookies = cookiesEl.value;
  }).catch(() => {});
  invoke('get_tools_status').then(showTools).catch(() => {});
  renderQueue();
  urlEl.focus();
})();
