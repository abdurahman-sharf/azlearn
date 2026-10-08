// Applies the saved light/dark theme before the first paint (no flash). Kept as a separate file so the page
// can run under a Content-Security-Policy that forbids inline scripts.
(function () {
  try {
    var t = localStorage.getItem('exameow-theme');
    if (t !== 'system' && t !== 'light' && t !== 'dark') {
      var legacy = localStorage.getItem('exameow-dark');
      t = legacy === '1' ? 'dark' : legacy === '0' ? 'light' : 'system';
    }
    var dark = t === 'dark' || (t === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
    document.documentElement.classList.toggle('dark', dark);
  } catch (e) {}
})();
