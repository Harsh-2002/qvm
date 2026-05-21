// qvm web UI — minimal vanilla JS.
//
// Responsibilities:
//   * 2-second auto-refresh of the VM grid (home page only)
//   * Overlay during long-running create POSTs
//   * Toast auto-dismiss after 5 seconds
//   * Keyboard shortcuts (n, /, ?, Esc)
//
// All page navigation is plain server-rendered HTML — no SPA.

(function () {
  // ── grid auto-refresh ──────────────────────────────────────────────────
  const grid = document.querySelector('[data-vm-grid]');
  if (grid) {
    let timer;
    async function refresh() {
      try {
        const res = await fetch('/?fragment=grid', { cache: 'no-store' });
        if (!res.ok) return;
        const html = await res.text();
        if (html.trim() !== grid.innerHTML.trim()) {
          grid.innerHTML = html;
        }
      } catch (_) { /* network blip, retry next tick */ }
    }
    function loop() { refresh(); timer = setTimeout(loop, 2000); }
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible') { clearTimeout(timer); loop(); }
      else { clearTimeout(timer); }
    });
    loop();
  }

  // ── toast auto-dismiss ─────────────────────────────────────────────────
  const toast = document.getElementById('toast');
  if (toast) {
    setTimeout(() => {
      toast.classList.add('dismissing');
      setTimeout(() => toast.remove(), 220);
    }, 5000);
  }

  // ── create-form overlay ────────────────────────────────────────────────
  // The submit can take 30–60 s if a base image needs to be downloaded.
  // Show a full-screen spinner with the VM name so the user knows
  // something is happening.
  const createForm = document.querySelector('form[data-form="create"]');
  if (createForm) {
    createForm.addEventListener('submit', () => {
      const name = createForm.querySelector('input[name="name"]').value || 'VM';
      const distroSel = createForm.querySelector('select[name="distro"]');
      const distro = distroSel ? distroSel.value : '';
      const willPull = distroSel && distroSel.selectedOptions[0].textContent.includes('not pulled');
      const detail = willPull
        ? `Downloading ${distro} base image (~500 MB – 1 GB) and provisioning. This typically takes 1–2 minutes on a fast link.`
        : `Provisioning. This typically takes 5–15 seconds.`;
      showOverlay(`Creating ${name}…`, detail);
      // Let the form submit proceed normally; the browser navigates after
      // the server's 303 redirect. The overlay sits on top until then.
    });
  }

  // ── confirm-delete + lifecycle action overlays ─────────────────────────
  // Lifecycle actions (start/stop/restart) take a few seconds. Show a
  // brief overlay so the click feels responsive rather than dead.
  document.querySelectorAll('form[action^="/vm/"]:not([data-form="create"])').forEach(f => {
    f.addEventListener('submit', () => {
      const action = (f.getAttribute('action') || '').split('/').pop();
      const verb = ({
        start: 'Starting', stop: 'Stopping', restart: 'Restarting',
        delete: 'Deleting', kill: 'Killing',
      })[action] || 'Working';
      const m = f.action.match(/\/vm\/([^/]+)/);
      const name = m ? decodeURIComponent(m[1]) : '';
      showOverlay(`${verb} ${name}…`, '');
    });
  });

  function showOverlay(title, msg) {
    if (document.querySelector('.overlay')) return;
    const div = document.createElement('div');
    div.className = 'overlay';
    div.innerHTML =
      '<div class="overlay-inner">' +
      '<div class="spinner"></div>' +
      '<div class="overlay-title"></div>' +
      '<div class="overlay-msg"></div>' +
      '</div>';
    div.querySelector('.overlay-title').textContent = title;
    div.querySelector('.overlay-msg').textContent   = msg;
    document.body.appendChild(div);
  }

  // ── keyboard shortcuts ─────────────────────────────────────────────────
  // Don't hijack keys while typing in an input/textarea/select.
  function isEditing() {
    const a = document.activeElement;
    if (!a) return false;
    const tag = a.tagName;
    return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || a.isContentEditable;
  }
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (isEditing()) {
      // Inside an input, only intercept Escape (close modal / go home).
      if (e.key === 'Escape') {
        const modal = document.getElementById('help-modal');
        if (modal) { modal.remove(); e.preventDefault(); }
      }
      return;
    }
    switch (e.key) {
      case 'n':
        location.href = '/vm/new'; e.preventDefault();
        break;
      case '?':
        toggleHelp(); e.preventDefault();
        break;
      case 'Escape': {
        const modal = document.getElementById('help-modal');
        if (modal) { modal.remove(); }
        else if (location.pathname !== '/') location.href = '/';
        break;
      }
    }
  });

  function toggleHelp() {
    const existing = document.getElementById('help-modal');
    if (existing) { existing.remove(); return; }
    const m = document.createElement('div');
    m.id = 'help-modal';
    m.className = 'modal';
    m.innerHTML =
      '<div class="modal-inner">' +
        '<h2>Keyboard shortcuts</h2>' +
        '<table>' +
          '<tr><td><kbd>n</kbd></td><td>Create new VM</td></tr>' +
          '<tr><td><kbd>?</kbd></td><td>Toggle this help</td></tr>' +
          '<tr><td><kbd>Esc</kbd></td><td>Close modal · go home</td></tr>' +
        '</table>' +
        '<p class="hint" style="margin-top:1rem">Click anywhere to dismiss.</p>' +
      '</div>';
    m.addEventListener('click', () => m.remove());
    document.body.appendChild(m);
  }
})();
