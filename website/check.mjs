import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { resolve, relative, dirname, sep } from 'node:path';
import { build, outputRoot } from './build.mjs';

await build();

async function filesIn(directory) {
  const result = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) result.push(...await filesIn(path));
    else result.push(path);
  }
  return result;
}

const files = await filesIn(outputRoot);
const pages = new Map();
for (const file of files.filter((file) => file.endsWith('.html'))) {
  const html = await readFile(file, 'utf8');
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]);
  assert.equal(ids.length, new Set(ids).size, `${relative(outputRoot, file)} has duplicate HTML IDs`);
  assert(!/\{\{(?:head|header|footer|search|version|icon:|operation-|quickstart-)/.test(html), `${file} contains unrendered template tokens`);
  assert.equal([...html.matchAll(/<h1(?:\s|>)/g)].length, 1, `${file} must have one h1`);
  pages.set(file, { html, ids: new Set(ids) });
}

let linkCount = 0;
const broken = [];
for (const [file, { html }] of pages) {
  for (const [, href] of html.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
    if (/^(?:[a-z]+:|\/\/)/i.test(href)) continue;
    if (href.startsWith('/')) { broken.push(`${relative(outputRoot, file)} → ${href}: root-relative URL breaks project hosting`); continue; }
    const [pathname, hash] = href.split('#');
    const target = pathname.split('?')[0] ? resolve(dirname(file), decodeURIComponent(pathname.split('?')[0])) : file;
    const local = relative(outputRoot, target);
    if (local.startsWith('..' + sep) || local === '..') { broken.push(`${file} → ${href}: escapes build output`); continue; }
    try {
      await stat(target);
      if (hash && pages.has(target) && !pages.get(target).ids.has(decodeURIComponent(hash))) broken.push(`${relative(outputRoot, file)} → ${href}: missing anchor`);
      linkCount += 1;
    } catch { broken.push(`${relative(outputRoot, file)} → ${href}: missing file`); }
  }
  for (const [, controls] of html.matchAll(/aria-controls="([^"]+)"/g)) {
    for (const id of controls.split(' ')) assert(pages.get(file).ids.has(id), `${file} references missing control ${id}`);
  }
}
assert.deepEqual(broken, [], `Broken website links:\n${broken.join('\n')}`);
const search = JSON.parse(await readFile(resolve(outputRoot, 'search-index.json'), 'utf8'));
for (const page of search) assert(pages.has(resolve(outputRoot, page.path)), `Missing search result: ${page.path}`);
assert.equal(search.length, pages.size - 2, 'Every documentation page must be searchable');
for (const term of ['federation', 'caching', 'authentication', 'quick start']) assert(search.some((page) => page.title.toLowerCase().includes(term)), `Search is missing ${term}`);
console.log(`Checked ${pages.size} HTML pages, ${linkCount} local links/assets, all control targets, and ${search.length} search entries.`);
