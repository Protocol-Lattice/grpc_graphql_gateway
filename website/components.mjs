export const repository = 'https://github.com/Protocol-Lattice/grpc_graphql_gateway';

export const escapeHTML = (value = '') => String(value).replace(/[&<>"']/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]);

const icons = {
  arrow: '<path d="M4 12h16m-6-6 6 6-6 6"/>',
  external: '<path d="M7 17 17 7M7 7h10v10"/>',
  search: '<circle cx="10.8" cy="10.8" r="6.8"/><path d="m16 16 4.5 4.5"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M15 8V4H4v11h4"/>',
  github: '<path d="M9 19c-4.5 1.5-4.5-2.5-6-3m12 6v-4a3.5 3.5 0 0 0-1-2.7c3.3-.4 6.7-1.6 6.7-7.3a5.7 5.7 0 0 0-1.5-4A5.3 5.3 0 0 0 19.1 0S17.9-.4 15 1.5a13.8 13.8 0 0 0-6 0C6.1-.4 4.9 0 4.9 0a5.3 5.3 0 0 0-.1 4A5.7 5.7 0 0 0 3.3 8c0 5.7 3.4 6.9 6.7 7.3A3.5 3.5 0 0 0 9 18v4" transform="translate(0 1) scale(1 .9)"/>',
  layers: '<path d="m12 3 10 6-10 6L2 9l10-6Zm-10 12 10 6 10-6M2 12l10 6 10-6"/>',
  network: '<rect x="9" y="2" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="16" y="16" width="6" height="6" rx="1"/><path d="M12 8v4M5 16v-4h14v4"/>',
  shield: '<path d="m12 2 8 4v6c0 5-8 10-8 10S4 17 4 12V6l8-4Z"/><path d="m8 12 3 3 5-6"/>',
  bolt: '<path d="m13 2-9 12h7l-1 8 10-13h-7l1-7Z"/>',
  code: '<path d="m7 7-5 5 5 5m10-10 5 5-5 5M14 3l-4 18"/>',
  pulse: '<path d="M2 12h5l3-8 4 16 3-8h5"/>',
  book: '<path d="M12 5v16M2 3h6a4 4 0 0 1 4 2 4 4 0 0 1 4-2h6v16h-6a4 4 0 0 0-4 2 4 4 0 0 0-4-2H2V3Z"/>',
  menu: '<path d="M4 6h16M4 12h16M4 18h16"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
  terminal: '<path d="m4 6 6 6-6 6m8 0h8"/>',
};

export function icon(name) {
  return `<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[name] || icons.arrow}</svg>`;
}

export function brand(root) {
  return `<a class="brand" href="${root}index.html" aria-label="gRPC GraphQL Gateway home"><span class="brand-symbol"><img src="${root}assets/gateway-logo.svg" width="1254" height="1254" alt=""></span><span>grpc<span class="brand-divider">/</span>graphql<span class="brand-subtitle">GATEWAY</span></span></a>`;
}

export function header(root, docs = false) {
  return `<a class="skip-link" href="#main">Skip to content</a>
  <header class="site-header"><div class="container header-inner">${brand(root)}
    <nav class="desktop-nav" aria-label="Main navigation"><a href="${root}index.html#features">Why Gateway</a><a href="${root}index.html#explore">How it works</a><a href="${root}introduction.html"${docs ? ' aria-current="true"' : ''}>Documentation ${icon('external')}</a></nav>
    <div class="header-actions"><button class="search-trigger" data-search-open aria-label="Search documentation">${icon('search')}<kbd>⌘ K</kbd></button><a class="github-link" href="${repository}" aria-label="View the project on GitHub">${icon('github')}</a><a class="button button-small button-outline header-cta" href="${root}getting-started/quick-start.html">Get started ${icon('external')}</a><button class="menu-toggle" aria-expanded="false" aria-controls="mobile-navigation" aria-label="Open navigation">${icon('menu')}</button></div>
    </div><nav class="mobile-nav" id="mobile-navigation" aria-label="Mobile navigation" hidden><a href="${root}index.html#features">Why Gateway</a><a href="${root}index.html#explore">How it works</a><a href="${root}introduction.html">Documentation</a><a href="${root}getting-started/quick-start.html">Get started</a></nav>
  </header>`;
}

export function footer(root) {
  return `<footer class="site-footer"><div class="container footer-inner"><div>${brand(root)}<p>Built by Protocol Lattice. Built in the open.</p></div><nav aria-label="Footer navigation"><a href="${root}introduction.html">Documentation</a><a href="${repository}">GitHub ${icon('external')}</a><a href="${repository}/blob/main/LICENSE">MIT License ${icon('external')}</a></nav><span class="footer-note">Less glue. More GraphQL.</span></div></footer>`;
}

export function searchDialog() {
  return `<dialog class="search-dialog" aria-labelledby="search-title"><div class="search-input-wrap">${icon('search')}<label class="sr-only" id="search-title" for="docs-search">Search documentation</label><input id="docs-search" type="search" placeholder="Search the documentation…" autocomplete="off" spellcheck="false" aria-controls="search-results"><button class="search-close" aria-label="Close search">Esc</button></div><div class="search-results" id="search-results"></div><div class="search-help"><span><kbd>↑</kbd> <kbd>↓</kbd> navigate <kbd>↵</kbd> open</span><span>Documentation, at your fingertips.</span></div></dialog><div class="toast" role="status" aria-live="polite"></div>`;
}

export function head({ title, description, root }) {
  return `<meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="theme-color" content="#101313"><meta name="description" content="${escapeHTML(description)}"><meta property="og:title" content="${escapeHTML(title)}"><meta property="og:description" content="${escapeHTML(description)}"><meta property="og:type" content="website"><title>${escapeHTML(title)}</title><link rel="icon" href="${root}assets/gateway-logo.svg" type="image/svg+xml"><link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin><link href="https://fonts.googleapis.com/css2?family=DM+Sans:wght@400;450;500;550;600;650;700;750;800&family=IBM+Plex+Mono:wght@400;450;500&display=swap" rel="stylesheet"><link rel="stylesheet" href="${root}styles.css"><script type="module" src="${root}app.js"></script>`;
}
