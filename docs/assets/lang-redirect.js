// Auto-redirect to Chinese version based on browser language
(function() {
  if (window.location.pathname !== '/' && window.location.pathname !== '') return;
  // Already redirected this session — user made a choice, respect it
  if (sessionStorage.getItem('lang-redirected')) return;
  var lang = navigator.language || navigator.userLanguage || '';
  if (lang.startsWith('zh')) {
    sessionStorage.setItem('lang-redirected', '1');
    window.location.replace('/zh/');
  }
})();
