import { cp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { dirname, join, posix, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Marked, Renderer } from 'marked';
import hljs from 'highlight.js';
import { escapeHTML, footer, head, header, icon, repository, searchDialog } from './components.mjs';
import { operations, quickstart } from './examples.mjs';

export const projectRoot = fileURLToPath(new URL('../', import.meta.url));
export const outputRoot = join(projectRoot, 'website/dist');
const sourceRoot = join(projectRoot, 'docs/src');
const publicRoot = join(projectRoot, 'website/public');

const highlight = (code, language) => hljs.getLanguage(language) ? hljs.highlight(code, { language }).value : escapeHTML(code);
const cleanText = (html) => html.replace(/<[^>]*>/g, '').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/\p{Extended_Pictographic}\uFE0F?/gu, '').trim();

function codeWindow(code, language, filename = language || 'Code', numbered = false) {
  const formatted = highlight(code, language);
  // Number each line without inserting text nodes that add blank lines in <pre>.
  const body = numbered ? formatted.split('\n').map((line) => `<span class="code-line">${line || ' '}</span>`).join('') : formatted;
  return `<div class="code-window"><div class="code-toolbar"><span class="code-toolbar-label"><span class="file-dot${language === 'graphql' ? ' pink' : ''}"></span>${escapeHTML(filename)}</span><button class="copy-button copy-code" aria-label="Copy ${escapeHTML(filename)}">${icon('copy')}</button></div><pre tabindex="0" aria-label="${escapeHTML(filename)} code"><code${numbered ? ' class="numbered-code"' : ''}>${body}</code></pre></div>`;
}

function operationMarkup() {
  const tabs = `<div class="operation-tabs" role="tablist" aria-label="GraphQL operation">${operations.map((operation, index) => `<button type="button" role="tab" id="operation-${operation.id}" aria-controls="panel-${operation.id}" aria-selected="${index === 0}" tabindex="${index === 0 ? '0' : '-1'}">${icon(operation.icon)}${operation.label}</button>`).join('')}</div>`;
  const panels = operations.map((operation, index) => {
    const input = codeWindow(operation.proto, operation.language, operation.filename, true);
    const graphql = codeWindow(operation.graphql, 'graphql', operation.id === 'federation' ? 'generated-schema.graphql' : `${operation.id}.graphql`, true);
    return `<div class="operation-content" role="tabpanel" id="panel-${operation.id}" aria-labelledby="operation-${operation.id}"${index ? ' hidden' : ''}><div class="operation-info"><span class="operation-eyebrow">${operation.eyebrow}</span><h3>${operation.title}</h3><p>${operation.description}</p><div class="operation-tags">${operation.tags.map((tag) => `<span>${tag}</span>`).join('')}</div><a class="text-link" href="./${operation.docs}">Read the guide ${icon('external')}</a></div><div class="operation-code">${input}<div class="translation-divider">${icon('arrow')} ${operation.id === 'federation' ? 'GENERATED FEDERATION TYPE' : 'EXPOSED AS GRAPHQL'}</div>${graphql}<div class="demo-actions"><span>INTERACTIVE EXAMPLE</span><button class="demo-button" aria-expanded="false" aria-controls="response-${operation.id}">Show example response</button></div><div class="example-response" id="response-${operation.id}" hidden><pre tabindex="0" aria-label="Example response"><code>${highlight(JSON.stringify(operation.result, null, 2), 'json')}</code></pre><p>${operation.note}</p></div></div></div>`;
  }).join('');
  return { tabs, panels };
}

function quickstartMarkup(version) {
  const tabs = `<div class="quick-tabs" role="tablist" aria-label="Quick start files">${quickstart.map((example, index) => `<button type="button" role="tab" id="quick-${example.id}" aria-controls="quick-panel-${example.id}" aria-selected="${index === 0}" tabindex="${index === 0 ? '0' : '-1'}">${example.label}</button>`).join('')}</div>`;
  const panels = quickstart.map((example, index) => `<div class="quick-panel" role="tabpanel" id="quick-panel-${example.id}" aria-labelledby="quick-${example.id}"${index ? ' hidden' : ''}>${codeWindow(example.code.replaceAll('{{version}}', version), example.language, example.label, true)}</div>`).join('');
  return { tabs, panels };
}

async function markdownFiles(directory, prefix = '') {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = posix.join(prefix, entry.name);
    if (entry.isDirectory()) files.push(...await markdownFiles(join(directory, entry.name), path));
    else if (entry.name.endsWith('.md') && entry.name !== 'SUMMARY.md') files.push(path);
  }
  return files.sort();
}

function documentNavigation(summary) {
  let group = 'Overview';
  const pages = [];
  for (const line of summary.split('\n')) {
    if (/^# /.test(line) && line !== '# Summary') group = line.slice(2).trim();
    const match = line.match(/\[([^\]]+)\]\(([^)]+\.md)\)/);
    if (match) pages.push({ title: match[1], source: posix.normalize(match[2]), group });
  }
  return pages;
}

function renderDocument(markdown, source, documentPaths) {
  const toc = [];
  const ids = new Map();
  const renderer = new Renderer();
  const defaultTable = renderer.table;
  const defaultLink = renderer.link;
  renderer.heading = function ({ tokens, depth }) {
    const html = this.parser.parseInline(tokens).replace(/\p{Extended_Pictographic}\uFE0F?/gu, '').trim();
    const label = cleanText(html);
    const slug = label.toLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, '').replace(/\s/g, '-').replace(/^-|-$/g, '') || 'section';
    const occurrence = ids.get(slug) || 0;
    ids.set(slug, occurrence + 1);
    const id = occurrence ? `${slug}-${occurrence}` : slug;
    toc.push({ id, label, depth });
    return `<h${depth} id="${escapeHTML(id)}"><a class="heading-anchor" href="#${escapeHTML(id)}">${html}</a></h${depth}>\n`;
  };
  renderer.code = ({ text, lang }) => codeWindow(text, (lang || '').split(/[ ,]/)[0]);
  renderer.table = function (token) { return `<div class="table-wrap">${defaultTable.call(this, token)}</div>`; };
  renderer.link = function (token) {
    let href = token.href;
    if (!/^(?:[a-z]+:|\/\/|#)/i.test(href)) {
      const [path, suffix = ''] = href.split(/(?=[?#])/s, 2);
      const target = posix.normalize(posix.join(posix.dirname(source), path));
      if (documentPaths.has(target)) href = path.replace(/\.md$/, '.html') + suffix;
      else if (path && !path.endsWith('.html')) href = `${repository}/blob/main/${posix.normalize(posix.join('docs/src', target))}${suffix}`;
    }
    return defaultLink.call(this, { ...token, href });
  };
  const parser = new Marked({ renderer, gfm: true });
  return { content: parser.parse(markdown), toc };
}

function sidebar(navigation, source, root) {
  const groups = [...new Set(navigation.map((page) => page.group))];
  return `<aside class="docs-sidebar" id="docs-sidebar"><a class="docs-home-link" href="${root}index.html">${icon('arrow')} Back to Gateway</a><nav aria-label="Documentation">${groups.map((group) => `<div class="docs-nav-group"><h2>${escapeHTML(group)}</h2>${navigation.filter((page) => page.group === group).map((page) => `<a href="${root}${page.source.replace(/\.md$/, '.html')}"${source === page.source ? ' aria-current="page"' : ''}>${escapeHTML(page.title)}</a>`).join('')}</div>`).join('')}</nav></aside>`;
}

function pagination(navigation, source, root) {
  const current = navigation.findIndex((page) => page.source === source);
  if (current < 0) return '';
  return `<nav class="doc-pagination" aria-label="Documentation pages">${[navigation[current - 1], navigation[current + 1]].map((page, index) => page ? `<a href="${root}${page.source.replace(/\.md$/, '.html')}"><small>${index ? 'UP NEXT →' : '← PREVIOUS'}</small>${escapeHTML(page.title)}</a>` : '<span></span>').join('')}</nav>`;
}

export async function build() {
  const [cargo, summary, landingTemplate, documents] = await Promise.all([
    readFile(join(projectRoot, 'Cargo.toml'), 'utf8'),
    readFile(join(sourceRoot, 'SUMMARY.md'), 'utf8'),
    readFile(join(publicRoot, 'index.html'), 'utf8'),
    markdownFiles(sourceRoot),
  ]);
  const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error('Could not read the gateway version from Cargo.toml.');
  const navigation = documentNavigation(summary);
  const documentPaths = new Set(documents);
  for (const page of navigation) if (!documentPaths.has(page.source)) throw new Error(`Missing documentation source: ${page.source}`);

  // This output directory contains generated website files only.
  await rm(outputRoot, { recursive: true, force: true });
  await mkdir(outputRoot, { recursive: true });
  await cp(publicRoot, outputRoot, { recursive: true });
  const operation = operationMarkup();
  const quick = quickstartMarkup(version);
  const replacements = {
    head: head({ title: 'gRPC–GraphQL Gateway — Many services. One graph.', description: 'Turn your gRPC services into a unified GraphQL API. A Rust gateway with generated schemas, real-time subscriptions, and Apollo Federation v2.', root: './' }),
    header: header('./'), footer: footer('./'), search: searchDialog(), version,
    'operation-tabs': operation.tabs, 'operation-panels': operation.panels,
    'quickstart-tabs': quick.tabs, 'quickstart-panels': quick.panels,
  };
  const landing = landingTemplate.replace(/\{\{([^}]+)\}\}/g, (_, key) => {
    if (key.startsWith('icon:')) return icon(key.slice(5));
    if (!(key in replacements)) throw new Error(`Unknown template token: ${key}`);
    return replacements[key];
  });
  await writeFile(join(outputRoot, 'index.html'), landing);

  const searchIndex = [];
  for (const source of documents) {
    let markdown = await readFile(join(sourceRoot, source), 'utf8');
    // Replace the old mdBook introduction banner with a semantic document heading.
    if (source === 'introduction.md') markdown = markdown.replace(/<div class="hero-section">[\s\S]*?<\/div>\s*<\/div>/, '# gRPC-GraphQL Gateway\n');
    const { content, toc } = renderDocument(markdown, source, documentPaths);
    const page = navigation.find((entry) => entry.source === source);
    const title = toc.find((entry) => entry.depth === 1)?.label || page?.title || source;
    const group = page?.group || 'Documentation';
    const description = cleanText(content).replace(/\s+/g, ' ').slice(0, 180);
    const path = source.replace(/\.md$/, '.html');
    const root = '../'.repeat(path.split('/').length - 1) || './';
    const html = `<!doctype html><html lang="en"><head>${head({ title: `${title} — gRPC–GraphQL Gateway`, description, root })}</head><body class="docs-page">${header(root, true)}<button class="docs-mobile-toggle" aria-expanded="false" aria-controls="docs-sidebar">Browse documentation ${icon('menu')}</button><div class="docs-layout">${sidebar(navigation, source, root)}<main class="doc-content" id="main"><div class="doc-breadcrumb"><a href="${root}introduction.html">Docs</a><span>/</span><span>${escapeHTML(group)}</span></div>${content}<div class="doc-edit"><a href="${repository}/edit/main/docs/src/${source}">Improve this page on GitHub ${icon('external')}</a></div>${pagination(navigation, source, root)}</main><aside class="docs-toc" aria-label="On this page"><span>ON THIS PAGE</span>${toc.filter((entry) => entry.depth === 2 || entry.depth === 3).map((entry) => `<a href="#${escapeHTML(entry.id)}"${entry.depth === 3 ? ' class="toc-subheading"' : ''}>${escapeHTML(entry.label)}</a>`).join('')}</aside></div>${footer(root)}${searchDialog()}</body></html>`;
    await mkdir(dirname(join(outputRoot, path)), { recursive: true });
    await writeFile(join(outputRoot, path), html);
    searchIndex.push({ title, group, path, description, text: cleanText(content).replace(/\s+/g, ' ') });
  }
  await writeFile(join(outputRoot, 'search-index.json'), JSON.stringify(searchIndex));
  await writeFile(join(outputRoot, '.nojekyll'), '');
  await writeFile(join(outputRoot, '404.html'), `<!doctype html><html lang="en"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Page not found — Gateway</title><style>body{background:#101313;color:#f1f3ee;font:16px/1.7 system-ui;margin:0;display:grid;place-content:center;min-height:100vh;text-align:center;padding:24px;box-sizing:border-box}h1{font-size:48px;letter-spacing:-2px;margin:0}p{color:#9aa7a1}a{color:#7ce2bc}</style></head><body><h1>This connection is missing.</h1><p>The page may have moved, but the documentation is still here.</p><a href="https://protocol-lattice.github.io/grpc_graphql_gateway/">Back to Gateway →</a></body></html>`);
  console.log(`Built Gateway v${version}: landing page + ${documents.length} documentation pages in website/dist.`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await build();
