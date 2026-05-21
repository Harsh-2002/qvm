// qvm web UI — minimal vanilla JS.
//
// Refreshes the VM list every 2s without a full page reload. Other
// pages are server-rendered, no JS framework needed.

(function () {
  const grid = document.querySelector('[data-vm-grid]');
  if (!grid) return;

  let timer;
  async function refresh() {
    try {
      const res = await fetch('/?fragment=grid', { cache: 'no-store' });
      if (!res.ok) return;
      const html = await res.text();
      // Only swap if the content actually changed — avoid disrupting hover/focus.
      if (html.trim() !== grid.innerHTML.trim()) {
        grid.innerHTML = html;
      }
    } catch (_) { /* network blip, try again next tick */ }
  }
  function loop() { refresh(); timer = setTimeout(loop, 2000); }
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') { clearTimeout(timer); loop(); }
    else { clearTimeout(timer); }
  });
  loop();
})();
