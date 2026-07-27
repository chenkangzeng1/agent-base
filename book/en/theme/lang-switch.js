(function () {
  var path = window.location.pathname;
  var ptr = typeof path_to_root !== 'undefined' ? path_to_root : '';

  // Detect ZH pages: path contains /zh/ as a directory segment
  var isZh = /\/zh(\/|$)/.test(path);

  var targetPath;

  // Compute book root by resolving path_to_root against current directory
  var dir = path.replace(/\/[^/]*$/, '/');
  var a = document.createElement('a');
  a.href = dir + ptr;
  var bookRoot = a.pathname;

  // Page path relative to the book root
  var relPath = path.startsWith(bookRoot) ? path.slice(bookRoot.length) : path;

  if (isZh) {
    // Remove /zh segment from the original path
    targetPath = path.replace(/\/zh(?=\/|$)/, '').replace(/\/+$/, '') || '/';
  } else {
    targetPath = bookRoot + 'zh/' + relPath;
  }

  var label = isZh ? 'English' : '中文';
  var lang = isZh ? 'en' : 'zh';

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
