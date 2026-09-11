const all = (selector, root = document) => [...root.querySelectorAll(selector)];
const byId = (id) => document.getElementById(id);
const toast = document.querySelector('.toast');
let toastTimer;

function notify(message) {
  toast.textContent = message;
  toast.classList.add('visible');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove('visible'), 2600);
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const input = document.createElement('textarea');
    input.value = text;
    input.style.cssText = 'position:fixed;top:0;left:-9999px';
    input.setAttribute('aria-label', 'Copy code');
    const focused = document.activeElement;
    document.body.append(input);
    input.select();
    let copied = false;
    try { copied = document.execCommand('copy'); } catch { /* Manual copying remains available. */ }
    input.remove();
    focused?.focus();
    return copied;
  }
}

document.addEventListener('click', async (event) => {
  const button = event.target.closest('[data-copy], .copy-code');
  if (!button) return;
  const code = button.closest('.code-window')?.querySelector('code');
  const lines = code ? all('.code-line', code) : [];
  const text = button.dataset.copy ?? (lines.length ? lines.map((line) => line.textContent.trimEnd()).join('\n') : code?.textContent);
  if (!text) return;
  const copied = await copyText(text);
  notify(copied ? 'Copied to clipboard.' : 'Select the code and copy it with ⌘C or Ctrl+C.');
});

for (const tablist of all('[role="tablist"]')) {
  const tabs = all('[role="tab"]', tablist);
  function select(tab, focus = false) {
    for (const item of tabs) {
      const active = item === tab;
      item.setAttribute('aria-selected', String(active));
      item.tabIndex = active ? 0 : -1;
      byId(item.getAttribute('aria-controls')).hidden = !active;
    }
    if (focus) tab.focus();
  }
  for (const tab of tabs) {
    tab.addEventListener('click', () => select(tab));
    tab.addEventListener('keydown', (event) => {
      const index = tabs.indexOf(tab);
      const next = { ArrowRight: (index + 1) % tabs.length, ArrowLeft: (index + tabs.length - 1) % tabs.length, Home: 0, End: tabs.length - 1 }[event.key];
      if (next === undefined) return;
      event.preventDefault();
      select(tabs[next], true);
    });
  }
}

for (const button of all('.demo-button')) {
  button.addEventListener('click', () => {
    const open = button.getAttribute('aria-expanded') !== 'true';
    button.setAttribute('aria-expanded', String(open));
    button.textContent = open ? 'Hide example response' : 'Show example response';
    byId(button.getAttribute('aria-controls')).hidden = !open;
  });
}

const menuToggle = document.querySelector('.menu-toggle');
const mobileNavigation = byId('mobile-navigation');
function setMenu(open, focus = false) {
  menuToggle.setAttribute('aria-expanded', String(open));
  menuToggle.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation');
  mobileNavigation.hidden = !open;
  if (!open && focus) menuToggle.focus();
}
menuToggle?.addEventListener('click', () => setMenu(mobileNavigation.hidden));
mobileNavigation?.addEventListener('click', (event) => { if (event.target.closest('a')) setMenu(false); });

const sidebarToggle = document.querySelector('.docs-mobile-toggle');
const sidebar = byId('docs-sidebar');
function setSidebar(open) {
  sidebarToggle?.setAttribute('aria-expanded', String(open));
  sidebar?.classList.toggle('is-open', open);
}
sidebarToggle?.addEventListener('click', () => setSidebar(sidebarToggle.getAttribute('aria-expanded') !== 'true'));
document.addEventListener('click', (event) => {
  if (!event.target.closest('.site-header')) setMenu(false);
  if (!event.target.closest('.docs-sidebar, .docs-mobile-toggle')) setSidebar(false);
});
const desktop = matchMedia('(min-width: 681px)');
desktop.addEventListener('change', () => { setMenu(false); setSidebar(false); });

const dialog = document.querySelector('.search-dialog');
const input = byId('docs-search');
const results = byId('search-results');
let indexPromise;
let previousFocus;
let searchGeneration = 0;
const normalize = (text) => text.toLowerCase().normalize('NFKD').replace(/[\u0300-\u036f]/g, '');

function getSearchIndex() {
  if (!indexPromise) {
    indexPromise = fetch(new URL('./search-index.json', import.meta.url)).then((response) => {
      if (!response.ok) throw new Error('Search index unavailable');
      return response.json();
    }).then((pages) => pages.map((page) => ({ ...page, searchable: normalize(`${page.title} ${page.group} ${page.text}`), normalizedTitle: normalize(page.title) }))).catch((error) => { indexPromise = undefined; throw error; });
  }
  return indexPromise;
}

function searchMessage(message) {
  const paragraph = document.createElement('p');
  paragraph.className = 'search-message';
  paragraph.textContent = message;
  results.replaceChildren(paragraph);
}

async function renderResults() {
  const generation = ++searchGeneration;
  const term = normalize(input.value.trim());
  const terms = term.split(/\s+/).filter(Boolean);
  try {
    const pages = await getSearchIndex();
    if (generation !== searchGeneration) return;
    const defaultPages = ['getting-started/quick-start.html', 'getting-started/installation.html', 'core/operations.html', 'federation/overview.html', 'advanced/live-queries.html', 'production/helm-deployment.html'];
    const matches = terms.length
      ? pages.filter((page) => terms.every((word) => page.searchable.includes(word))).map((page) => ({ page, score: (page.normalizedTitle.includes(term) ? 100 : 0) + terms.filter((word) => page.normalizedTitle.includes(word)).length * 20 })).sort((a, b) => b.score - a.score).map(({ page }) => page).slice(0, 9)
      : defaultPages.map((path) => pages.find((page) => page.path === path)).filter(Boolean);
    if (!matches.length) { searchMessage(`No pages found for “${input.value.trim()}”. Try “federation”, “caching”, or “quick start”.`); return; }
    const fragment = document.createDocumentFragment();
    for (const page of matches) {
      const link = document.createElement('a');
      link.className = 'search-result';
      link.href = new URL(page.path, import.meta.url).href;
      const category = document.createElement('span');
      category.textContent = page.group.toUpperCase();
      const title = document.createElement('strong');
      title.textContent = page.title;
      const description = document.createElement('p');
      const position = terms.length ? page.searchable.indexOf(terms[0]) : -1;
      description.textContent = position > 200 ? `…${page.text.slice(Math.max(0, position - 80), position + 100)}…` : `${page.description.slice(0, 130)}…`;
      link.append(category, title, description);
      fragment.append(link);
    }
    results.replaceChildren(fragment);
  } catch {
    if (generation === searchGeneration) searchMessage('Search could not load. Please try again, or browse the documentation from the navigation.');
  }
}

function openSearch() {
  if (dialog.open) return;
  previousFocus = document.activeElement;
  setMenu(false);
  setSidebar(false);
  dialog.showModal();
  input.value = '';
  searchMessage('Loading documentation…');
  input.focus();
  renderResults();
}
for (const button of all('[data-search-open]')) button.addEventListener('click', openSearch);
input?.addEventListener('input', renderResults);
document.querySelector('.search-close')?.addEventListener('click', () => dialog.close());
dialog?.addEventListener('close', () => previousFocus?.focus());
dialog?.addEventListener('click', (event) => { if (event.target === dialog) dialog.close(); });
dialog?.addEventListener('keydown', (event) => {
  const links = all('.search-result', results);
  const focused = links.indexOf(document.activeElement);
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    if (!links.length) return;
    const index = event.key === 'ArrowDown' ? (focused + 1) % links.length : (focused < 0 ? links.length - 1 : (focused + links.length - 1) % links.length);
    links[index].focus();
  } else if (event.key === 'Enter' && document.activeElement === input && links.length) {
    event.preventDefault();
    links[0].click();
  }
});

document.addEventListener('keydown', (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); openSearch(); }
  if (event.key === 'Escape' && !dialog.open) {
    if (!mobileNavigation.hidden) setMenu(false, true);
    if (sidebar?.classList.contains('is-open')) { setSidebar(false); sidebarToggle.focus(); }
  }
});

// Follow the current section in the documentation table of contents.
const tocLinks = all('.docs-toc a');
if (tocLinks.length && 'IntersectionObserver' in window) {
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      for (const link of tocLinks) {
        if (link.hash === `#${entry.target.id}`) link.setAttribute('aria-current', 'location');
        else link.removeAttribute('aria-current');
      }
    }
  }, { rootMargin: '-100px 0px -65% 0px' });
  for (const link of tocLinks) { const heading = byId(decodeURIComponent(link.hash.slice(1))); if (heading) observer.observe(heading); }
}
