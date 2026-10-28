(function () {
  'use strict';

  var isEnglish = (document.documentElement.lang || '').toLowerCase().indexOf('en') === 0;
  var t = isEnglish ? {
    search: 'Search',
    closeSearch: 'Close search',
    placeholder: 'Search docs...',
    start: 'Type to search the docs.',
    tryText: 'Try: connection pool, firewall, KeepAlive.',
    noResults: 'No results for',
    loading: 'Loading index...',
    error: 'Search is unavailable right now.',
    hint: '↑↓ select · Enter open · Esc close'
  } : {
    search: '搜索',
    closeSearch: '关闭搜索',
    placeholder: '搜索文档…',
    start: '输入关键词开始搜索。',
    tryText: '试试搜索：连接池、防火墙、KeepAlive。',
    noResults: '没有找到结果：',
    loading: '正在加载索引…',
    error: '搜索暂时不可用。',
    hint: '↑↓ 选择 · Enter 打开 · Esc 关闭'
  };

  /* Client-side search */
  var searchDialog = null;
  var searchInput = null;
  var searchResults = null;
  var searchIndex = null;
  var searchLoading = false;
  var searchFailed = false;
  var searchActive = -1;
  var searchLinks = [];

  function buildSearch() {
    if (searchDialog) return;
    searchDialog = document.createElement('dialog');
    searchDialog.className = 'search-dialog';
    searchDialog.setAttribute('aria-label', t.search);
    searchDialog.innerHTML =
      '<div class="search-shell">' +
        '<div class="search-bar">' +
          '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><path d="m20 20-3.5-3.5"></path></svg>' +
          '<input class="search-input" id="search-input" type="search" autocomplete="off" spellcheck="false" aria-autocomplete="list" aria-controls="search-results">' +
          '<button class="search-close" type="button" aria-label="' + t.closeSearch + '">×</button>' +
        '</div>' +
        '<div class="search-results" id="search-results" role="listbox" aria-live="polite"></div>' +
        '<div class="search-footer"><span>' + t.hint + '</span><span><kbd>Esc</kbd></span></div>' +
      '</div>';
    document.body.appendChild(searchDialog);
    searchInput = searchDialog.querySelector('.search-input');
    searchResults = searchDialog.querySelector('.search-results');
    searchInput.placeholder = t.placeholder;
    searchInput.setAttribute('aria-label', t.search);
    searchDialog.querySelector('.search-close').addEventListener('click', function () {
      searchDialog.close();
    });
    searchDialog.addEventListener('click', function (event) {
      if (event.target === searchDialog) searchDialog.close();
    });
    searchDialog.addEventListener('close', function () {
      document.body.classList.remove('lb-open');
    });
    searchInput.addEventListener('input', function () {
      renderSearch(searchInput.value);
    });
    searchInput.addEventListener('keydown', function (event) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        moveSearch(1);
      } else if (event.key === 'ArrowUp') {
        event.preventDefault();
        moveSearch(-1);
      } else if (event.key === 'Enter' && searchActive >= 0 && searchLinks[searchActive]) {
        event.preventDefault();
        searchLinks[searchActive].click();
      }
    });
  }

  function loadSearchIndex() {
    if (searchIndex || searchLoading) return;
    searchLoading = true;
    searchFailed = false;
    fetch('/druid-rust/assets/search-index.json', { cache: 'no-cache' })
      .then(function (response) {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.json();
      })
      .then(function (data) {
        searchIndex = data;
        searchLoading = false;
        if (searchDialog && searchDialog.open) renderSearch(searchInput.value);
      })
      .catch(function () {
        searchLoading = false;
        searchFailed = true;
        if (searchDialog && searchDialog.open) renderSearch(searchInput.value);
      });
  }

  function emptySearchMessage(text) {
    searchResults.textContent = '';
    var node = document.createElement('div');
    node.className = 'search-empty';
    node.textContent = text;
    searchResults.appendChild(node);
  }

  function snippetFor(text, query) {
    text = String(text || '').replace(/\s+/g, ' ').trim();
    var token = query.toLowerCase().trim().split(/\s+/)[0] || '';
    var index = text.toLowerCase().indexOf(token);
    if (index < 0) return text.slice(0, 150) + (text.length > 150 ? '…' : '');
    var start = Math.max(0, index - 45);
    if (start > 0 && /[A-Za-z]/.test(text.charAt(start)) && /[A-Za-z]/.test(text.charAt(start - 1))) {
      var nextSpace = text.indexOf(' ', start);
      if (nextSpace !== -1 && nextSpace < index) start = nextSpace + 1;
    }
    var end = Math.min(text.length, index + 110);
    return (start > 0 ? '…' : '') + text.slice(start, end) + (end < text.length ? '…' : '');
  }

  function runSearch(query) {
    var q = query.trim().toLowerCase();
    if (!q || !searchIndex) return [];
    var lang = isEnglish ? 'en' : 'zh';
    var tokens = q.split(/\s+/);
    var found = [];
    searchIndex.forEach(function (entry) {
      if (entry.lang !== lang) return;
      entry.sections.forEach(function (section) {
        var haystack = (entry.title + ' ' + entry.description + ' ' + section.heading + ' ' + section.text).toLowerCase();
        var score = 0;
        for (var i = 0; i < tokens.length; i++) {
          var count = haystack.split(tokens[i]).length - 1;
          if (!count) return;
          score += Math.min(count, 5);
        }
        if (entry.title.toLowerCase().indexOf(q) !== -1) score += 10;
        if (section.heading.toLowerCase().indexOf(q) !== -1) score += 6;
        if (section.text.toLowerCase().indexOf(q) !== -1) score += 2;
        if (!section.anchor) score -= 2;
        found.push({ entry: entry, section: section, score: score });
      });
    });
    found.sort(function (a, b) { return b.score - a.score; });
    return found.slice(0, 8);
  }

  function renderSearch(query) {
    if (!searchResults) return;
    searchLinks = [];
    searchActive = -1;
    searchResults.textContent = '';
    if (searchLoading) {
      emptySearchMessage(t.loading);
      return;
    }
    if (searchFailed) {
      emptySearchMessage(t.error);
      return;
    }
    if (!query.trim()) {
      emptySearchMessage(t.start + ' ' + t.tryText);
      return;
    }
    var results = runSearch(query);
    if (!results.length) {
      var mark = isEnglish ? '"' : '“';
      var closeMark = isEnglish ? '"' : '”';
      emptySearchMessage(t.noResults + ' ' + mark + query.trim() + closeMark);
      return;
    }
    results.forEach(function (result) {
      var link = document.createElement('a');
      link.className = 'search-result';
      link.href = result.entry.page + (result.section.anchor ? '#' + result.section.anchor : '');
      link.setAttribute('role', 'option');
      link.id = 'search-option-' + searchLinks.length;
      var top = document.createElement('span');
      top.className = 'search-result-title';
      var page = document.createElement('span');
      page.className = 'search-result-page';
      page.textContent = result.entry.label;
      var heading = document.createElement('span');
      heading.textContent = result.section.heading;
      top.appendChild(page);
      top.appendChild(heading);
      var section = document.createElement('span');
      section.className = 'search-result-section';
      section.textContent = (!result.section.anchor || result.section.heading === result.entry.title)
        ? result.entry.description
        : result.entry.title;
      var snippet = document.createElement('span');
      snippet.className = 'search-result-snippet';
      snippet.textContent = snippetFor(result.section.text, query);
      link.appendChild(top);
      link.appendChild(section);
      link.appendChild(snippet);
      searchResults.appendChild(link);
      searchLinks.push(link);
    });
    setSearchActive(0, false);
  }

  function setSearchActive(index, scroll) {
    searchLinks.forEach(function (link) {
      link.classList.remove('is-active');
      link.removeAttribute('aria-selected');
    });
    if (!searchLinks.length) {
      searchActive = -1;
      if (searchInput) searchInput.removeAttribute('aria-activedescendant');
      return;
    }
    searchActive = (index + searchLinks.length) % searchLinks.length;
    var active = searchLinks[searchActive];
    active.classList.add('is-active');
    active.setAttribute('aria-selected', 'true');
    if (searchInput) searchInput.setAttribute('aria-activedescendant', active.id);
    if (scroll) active.scrollIntoView({ block: 'nearest' });
  }

  function moveSearch(delta) {
    if (searchLinks.length) setSearchActive(searchActive + delta, true);
  }

  function openSearch() {
    buildSearch();
    loadSearchIndex();
    if (typeof searchDialog.showModal === 'function' && !searchDialog.open) {
      searchDialog.showModal();
    }
    document.body.classList.add('lb-open');
    searchInput.value = '';
    renderSearch('');
    searchInput.focus();
  }

  var searchButton = document.querySelector('.search-btn');
  if (searchButton) searchButton.addEventListener('click', openSearch);

  document.addEventListener('keydown', function (event) {
    var target = event.target;
    var typing = target && (
      target.tagName === 'INPUT' ||
      target.tagName === 'TEXTAREA' ||
      target.tagName === 'SELECT' ||
      target.isContentEditable
    );
    if ((event.ctrlKey || event.metaKey) && String(event.key).toLowerCase() === 'k') {
      if (searchDialog && searchDialog.open) return;
      event.preventDefault();
      openSearch();
      return;
    }
    if (event.key === '/' && !typing && !(searchDialog && searchDialog.open)) {
      event.preventDefault();
      openSearch();
    }
  });


})();
