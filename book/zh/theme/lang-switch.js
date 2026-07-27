(function () {
  var path = window.location.pathname;
  var isZh = path.indexOf('/zh/') === 0 || path === '/zh/' || path === '/zh';

  var label, targetPath, lang;
  if (isZh) {
    targetPath = path.replace(/^\/zh/, '') || '/';
    label = 'English';
    lang = 'en';
  } else {
    targetPath = '/zh' + path;
    label = '中文';
    lang = 'zh';
  }

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
