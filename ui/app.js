(function () {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const $ = (id) => document.getElementById(id);
  const rack = $('rack'), urlEl = $('url'), go = $('go'), fill = $('fill'),
        pct = $('pct'), msg = $('msg'), readout = $('readout'), dl = $('dl'),
        outdirEl = $('outdir'), pick = $('pick'), fmtWrap = document.querySelector('.fmt');
  const fmtBtns = [...document.querySelectorAll('.fmt button')];

  let fmt = 'mp3';
  let running = false;
  let revealPath = null;

  fmtBtns.forEach((b) => b.addEventListener('click', () => {
    fmt = b.dataset.fmt;
    fmtBtns.forEach((x) => x.classList.toggle('on', x === b));
  }));

  function setReadout(text, cls) {
    readout.textContent = text;
    readout.className = 'readout' + (cls ? ' ' + cls : '');
  }
  function setProgress(p, m) {
    fill.style.width = p + '%';
    pct.textContent = p + '%';
    if (m) msg.textContent = m;
  }
  function setRunning(on) {
    running = on;
    rack.classList.toggle('busy', on);
    go.textContent = on ? 'Annulla' : 'Estrai audio';
    go.classList.toggle('stop', on);
    pick.disabled = on;
    fmtWrap.classList.toggle('locked', on);
  }
  function fail(text) {
    setRunning(false);
    setReadout('ERRORE: ' + text, 'err');
    msg.textContent = 'Errore.';
  }
  function showOutDir(dir) {
    outdirEl.textContent = dir;
    outdirEl.title = dir;
  }

  function apply(s) {
    if (s.status === 'running') {
      setProgress(s.progress, s.message);
      if (s.title) setReadout('▸ ' + s.title.toUpperCase());
    } else if (s.status === 'done') {
      setRunning(false);
      setProgress(100, s.files > 1 ? 'Completato: ' + s.files + ' brani.' : 'Completato.');
      setReadout(s.title ? '✓ ' + s.title.toUpperCase() : '✓ COMPLETATO', 'ok');
      revealPath = s.file || outdirEl.title;
      dl.classList.add('show');
    } else if (s.status === 'error') {
      fail(s.message || 'errore sconosciuto.');
    } else if (s.status === 'cancelled') {
      setRunning(false);
      setProgress(0, 'Annullato.');
      setReadout('ANNULLATO.');
    }
  }

  go.addEventListener('click', async () => {
    if (running) { invoke('cancel_download'); return; }
    const url = urlEl.value.trim();
    if (!url) { setReadout('ERRORE: INSERIRE UN URL.', 'err'); return; }
    dl.classList.remove('show');
    setRunning(true);
    setProgress(0, 'Avvio…');
    setReadout('CONNESSIONE…');
    try {
      await invoke('start_download', { url, fmt });
    } catch (e) {
      fail(String(e));
    }
  });

  pick.addEventListener('click', async () => {
    try {
      const dir = await invoke('pick_out_dir');
      if (dir) showOutDir(dir);
    } catch (e) {
      setReadout('ERRORE: ' + e, 'err');
    }
  });

  dl.addEventListener('click', () => {
    if (revealPath) invoke('reveal', { path: revealPath }).catch(() => {});
  });

  urlEl.addEventListener('keydown', (e) => { if (e.key === 'Enter') go.click(); });

  // Adatta il rack alla finestra: ingrandisce o rimpicciolisce tutto insieme.
  // offsetWidth/offsetHeight non risentono del transform, quindi nessun loop.
  function fit() {
    const s = Math.max(0.5, Math.min(3,
      (window.innerWidth - 32) / rack.offsetWidth,
      (window.innerHeight - 32) / rack.offsetHeight));
    rack.style.transform = 'scale(' + s + ')';
  }
  new ResizeObserver(fit).observe(rack);
  window.addEventListener('resize', fit);
  fit();

  listen('job-update', (e) => apply(e.payload));
  invoke('get_out_dir').then(showOutDir).catch(() => {});
  urlEl.focus();
})();
