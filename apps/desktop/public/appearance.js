// Run before the first paint, independently of the application bundle.
(() => {
  const root = document.documentElement;
  const params = new URLSearchParams(location.search);
  try {
    const theme = localStorage.getItem('dogcodes.ui.theme');
    if (theme === 'light' || theme === 'dark') root.dataset.theme = theme;
    const raw = localStorage.getItem('dogcodes.ui.scale');
    const scale = (raw && raw.trim() && Number.isFinite(Number(raw)) ? Math.max(70, Math.min(150, Math.round(Number(raw) / 10) * 10)) : 100);
    root.style.setProperty('--ui-scale', String(scale / 100));
    const stored = Number(localStorage.getItem('dogcodes.sidebar.width'));
    const width = stored >= 200 && Number.isFinite(stored) ? Math.min(stored, 480) : innerWidth <= 1180 ? 240 : 286;
    root.style.setProperty('--startup-sidebar-width', `${Math.min(width, Math.max(200, innerWidth - 580))}px`);
    root.dataset.sidebarCollapsed = localStorage.getItem('dogcodes.sidebar.collapsed') === 'true' ? 'true' : 'false';
  } catch { /* Appearance remains usable when storage is unavailable. */ }
  if (params.has('reviewTask')) root.dataset.sidebarCollapsed = 'true';
  // A bundle download failure must not leave an endless empty window.
  setTimeout(() => {
    const fallback = document.getElementById('initial-startup-help');
    if (fallback) {
      fallback.hidden = false;
      fallback.querySelector('button').addEventListener('click', () => location.reload());
    }
  }, 12000);
})();
