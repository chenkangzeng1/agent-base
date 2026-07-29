// Auto-redirect to Chinese version based on browser language
(function() {
  if (window.location.pathname !== '/' && window.location.pathname !== '') return;
  var lang = navigator.language || navigator.userLanguage || '';
  if (lang.startsWith('zh')) {
    window.location.replace('/zh/');
  }
})();
