(function () {
  'use strict';

  document.documentElement.classList.add('js');

  var reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* Mobile navigation */
  var toggle = document.querySelector('.nav-toggle');
  var nav = document.getElementById('site-nav');
  if (toggle && nav) {
    var setNav = function (open) {
      nav.classList.toggle('is-open', open);
      toggle.setAttribute('aria-expanded', String(open));
      toggle.setAttribute('aria-label', open ? '关闭导航' : '打开导航');
    };
    toggle.addEventListener('click', function () {
      setNav(!nav.classList.contains('is-open'));
    });
    nav.addEventListener('click', function (event) {
      if (event.target.closest('a')) setNav(false);
    });
    document.addEventListener('keydown', function (event) {
      if (event.key === 'Escape' && nav.classList.contains('is-open')) {
        setNav(false);
        toggle.focus();
      }
    });
    window.matchMedia('(min-width: 901px)').addEventListener('change', function (event) {
      if (event.matches) setNav(false);
    });
  }

  /* Copy code blocks */
  document.querySelectorAll('.copy-btn').forEach(function (button) {
    button.addEventListener('click', function () {
      var card = button.closest('.code-card');
      var code = card && card.querySelector('pre');
      if (!code) return;
      var text = code.innerText;
      var done = function () {
        var original = button.textContent;
        button.textContent = '已复制';
        window.setTimeout(function () { button.textContent = original; }, 1600);
      };
      var fallback = function () {
        var area = document.createElement('textarea');
        area.value = text;
        area.setAttribute('readonly', '');
        area.style.position = 'fixed';
        area.style.opacity = '0';
        document.body.appendChild(area);
        area.select();
        var copied = false;
        try { copied = document.execCommand('copy'); } catch (error) {}
        document.body.removeChild(area);
        if (copied) done();
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done, fallback);
        return;
      }
      fallback();
    });
  });

  /* Image lightbox */
  var lightbox = null;
  var lbImage, lbTitle, lbStage, lbZoomOut, lbZoomIn, lbClose;
  var baseWidth = 0;
  var zoom = 1;

  function buildLightbox() {
    if (lightbox) return;
    lightbox = document.createElement('dialog');
    lightbox.className = 'lightbox';
    lightbox.setAttribute('aria-label', '图片预览');
    lightbox.innerHTML =
      '<div class="lb-shell">' +
        '<div class="lb-bar">' +
          '<span class="lb-title"></span>' +
          '<button class="lb-btn lb-zoom-out" type="button" aria-label="缩小">−</button>' +
          '<button class="lb-btn lb-reset" type="button" aria-label="重置缩放">1:1</button>' +
          '<button class="lb-btn lb-zoom-in" type="button" aria-label="放大">+</button>' +
          '<button class="lb-btn lb-close" type="button" aria-label="关闭预览">×</button>' +
        '</div>' +
        '<div class="lb-stage"><img alt=""></div>' +
        '<div class="lb-hint">使用 + / − 缩放，Esc 关闭</div>' +
      '</div>';
    document.body.appendChild(lightbox);
    lbTitle = lightbox.querySelector('.lb-title');
    lbStage = lightbox.querySelector('.lb-stage');
    lbImage = lightbox.querySelector('.lb-stage img');
    lbZoomOut = lightbox.querySelector('.lb-zoom-out');
    lbZoomIn = lightbox.querySelector('.lb-zoom-in');
    lbClose = lightbox.querySelector('.lb-close');

    lbClose.addEventListener('click', function () { lightbox.close(); });
    lbZoomOut.addEventListener('click', function () { setZoom(zoom - 0.25); });
    lbZoomIn.addEventListener('click', function () { setZoom(zoom + 0.25); });
    lightbox.querySelector('.lb-reset').addEventListener('click', function () {
      setZoom(1);
      lbStage.scrollTop = 0;
      lbStage.scrollLeft = 0;
    });
    lbStage.addEventListener('click', function (event) {
      if (event.target === lbStage) lightbox.close();
    });
    lightbox.addEventListener('close', function () {
      document.body.classList.remove('lb-open');
      lbImage.removeAttribute('src');
    });
    lightbox.addEventListener('keydown', function (event) {
      if (event.key === '+' || event.key === '=') { event.preventDefault(); setZoom(zoom + 0.25); }
      if (event.key === '-') { event.preventDefault(); setZoom(zoom - 0.25); }
      if (event.key === '0') { event.preventDefault(); setZoom(1); }
    });
  }

  function fitImage() {
    if (!lbImage.naturalWidth) return;
    var maxW = Math.min(window.innerWidth - 48, 1500);
    var maxH = Math.max(220, window.innerHeight - 150);
    var isSvg = /\.svg($|\?)/i.test(lbImage.currentSrc || lbImage.src);
    var cap = isSvg ? 3 : 1;
    var scale = Math.min(cap, maxW / lbImage.naturalWidth, maxH / lbImage.naturalHeight);
    baseWidth = Math.max(120, lbImage.naturalWidth * scale);
    lbImage.style.height = 'auto';
    lbStage.scrollTop = 0;
    lbStage.scrollLeft = 0;
    setZoom(1);
  }

  window.addEventListener('resize', function () {
    if (lightbox && lightbox.open) fitImage();
  }, { passive: true });

  function setZoom(next) {
    if (!lbImage || !baseWidth) return;
    zoom = Math.min(3, Math.max(1, Math.round(next * 100) / 100));
    lbImage.style.width = Math.round(baseWidth * zoom) + 'px';
    lbStage.classList.toggle('is-zoomed', zoom > 1);
    lbZoomOut.disabled = zoom <= 1;
    lbZoomIn.disabled = zoom >= 3;
  }

  function openLightbox(src, title) {
    if (!src) return;
    if (!lightbox) buildLightbox();
    if (typeof lightbox.showModal !== 'function') return;
    lbTitle.textContent = title || '图片预览';
    lbImage.alt = title || '图片预览';
    lbImage.style.width = '';
    baseWidth = 0;
    zoom = 1;
    lbStage.classList.remove('is-zoomed');
    lbStage.scrollTop = 0;
    lbStage.scrollLeft = 0;
    lbImage.onload = fitImage;
    lbImage.src = src;
    if (lbImage.complete && lbImage.naturalWidth) fitImage();
    lightbox.showModal();
    document.body.classList.add('lb-open');
    lbClose.focus();
  }

  if (typeof HTMLDialogElement !== 'undefined' && typeof HTMLDialogElement.prototype.showModal === 'function') {
    document.querySelectorAll('.figure img, .bento-media img, .pet-card img, .principle img, .page-pet, .notfound-pet').forEach(function (img) {
      if (img.closest('a')) return;
      var link = document.createElement('a');
      link.className = 'zoom-link';
      link.href = img.getAttribute('src');
      link.setAttribute('data-lightbox', '');
      link.setAttribute('data-title', img.alt || '图片预览');
      img.parentNode.insertBefore(link, img);
      link.appendChild(img);
    });

    document.addEventListener('click', function (event) {
      var link = event.target.closest('a[data-lightbox]');
      if (!link) return;
      event.preventDefault();
      openLightbox(link.getAttribute('href'), link.getAttribute('data-title') || link.textContent.trim());
    });
  }

  /* Reveal sections once, no scroll listener */
  var revealItems = document.querySelectorAll('.reveal');
  if (reduceMotion || !('IntersectionObserver' in window)) {
    revealItems.forEach(function (item) { item.classList.add('is-in'); });
  } else {
    var revealObserver = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          entry.target.classList.add('is-in');
          revealObserver.unobserve(entry.target);
        }
      });
    }, { rootMargin: '0px 0px -8% 0px' });
    revealItems.forEach(function (item) { revealObserver.observe(item); });
  }

  /* Back to top: mascot only */
  var toTop = document.querySelector('.to-top');
  var sentinel = document.querySelector('.top-sentinel');
  if (toTop) {
    if (sentinel && 'IntersectionObserver' in window) {
      new IntersectionObserver(function (entries) {
        toTop.classList.toggle('is-visible', !entries[0].isIntersecting);
      }, { threshold: 0 }).observe(sentinel);
    } else {
      toTop.classList.add('is-visible');
    }
    toTop.addEventListener('click', function (event) {
      event.preventDefault();
      window.scrollTo({ top: 0, behavior: reduceMotion ? 'auto' : 'smooth' });
      var first = document.querySelector('.site-header .brand');
      if (first) first.focus({ preventScroll: true });
    });

    var petImage = toTop.querySelector('img');
    if (petImage) {
      var idleSrc = petImage.getAttribute('src');
      var happySrc = idleSrc.replace(/smile\.svg$/, 'joy.svg');
      if (happySrc !== idleSrc) {
        var preload = new Image();
        preload.src = happySrc;
        var swap = function (src) { petImage.src = src; };
        toTop.addEventListener('mouseenter', function () { swap(happySrc); });
        toTop.addEventListener('mouseleave', function () { swap(idleSrc); });
        toTop.addEventListener('focus', function () { swap(happySrc); });
        toTop.addEventListener('blur', function () { swap(idleSrc); });
      }
    }
  }
})();
