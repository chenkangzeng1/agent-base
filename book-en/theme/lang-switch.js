(function () {
  var path = window.location.pathname;
  var isEn = path.indexOf('/en/') !== -1;
  var isZh = path.indexOf('/zh/') !== -1;

  if (!isEn && !isZh) return;

  var targetPath = path.replace('/en/', '/x/').replace('/zh/', '/en/').replace('/x/', '/zh/');
  var label = isEn ? '中文' : 'English';
  var lang = isEn ? 'zh' : 'en';

  function inject() {
    var toolbar = document.querySelector('.right-buttons');
    if (!toolbar) return setTimeout(inject, 50);

    var a = document.createElement('a');
    a.href = targetPath;
    a.title = label;
    a.setAttribute('aria-label', label);
    a.style.cssText =
      'display:inline-flex;align-items:center;text-decoration:none;color:var(--icons);padding:4px;font-size:13px;font-weight:600;';
    a.textContent = lang;
    toolbar.insertBefore(a, toolbar.firstChild);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', function () {
      setTimeout(inject, 100);
    });
  } else {
    inject();
  }
})();
