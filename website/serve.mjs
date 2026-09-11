import { readFile, realpath, stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const { values } = parseArgs({ options: { port: { type: 'string', default: '4173' }, base: { type: 'string', default: '/' } } });
const port = Number(values.port);
if (!/^\d+$/.test(values.port) || port < 0 || port > 65535) throw new Error('--port must be an integer between 0 and 65535.');
if (!/^\/(?:[\w-]+\/)*$/.test(values.base)) throw new Error('--base must be / or a path such as /grpc_graphql_gateway/.');
const root = await realpath(fileURLToPath(new URL('./dist', import.meta.url)));
const types = { '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.json': 'application/json; charset=utf-8', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2' };
function withinRoot(path) {
  const local = relative(root, path);
  return local !== '..' && !local.startsWith('..' + sep) && !isAbsolute(local);
}

const server = createServer(async (request, response) => {
  const reply = (status, body, type = 'text/plain; charset=utf-8', headers = {}) => {
    response.writeHead(status, { 'Content-Type': type, 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', ...headers });
    response.end(request.method === 'HEAD' ? undefined : body);
  };
  if (!['GET', 'HEAD'].includes(request.method)) { reply(405, 'Method not allowed', undefined, { Allow: 'GET, HEAD' }); return; }
  let pathname;
  try {
    pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
    if (pathname.includes('\0')) throw new Error('Invalid path');
  } catch { reply(400, 'Bad request'); return; }
  if (values.base !== '/' && pathname === values.base.slice(0, -1)) { reply(308, '', undefined, { Location: values.base }); return; }
  if (!pathname.startsWith(values.base)) { reply(404, 'Not found'); return; }
  try {
    let path = resolve(root, pathname.slice(values.base.length) || '.');
    if (!withinRoot(path)) { reply(403, 'Forbidden'); return; }
    if ((await stat(path)).isDirectory()) path = resolve(path, 'index.html');
    path = await realpath(path);
    if (!withinRoot(path)) { reply(403, 'Forbidden'); return; }
    if (!(await stat(path)).isFile()) { reply(404, 'Not found'); return; }
    reply(200, await readFile(path), types[extname(path)] || 'application/octet-stream');
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ENOTDIR') reply(404, await readFile(resolve(root, '404.html')), types['.html']);
    else if (error.code === 'EACCES' || error.code === 'EPERM') reply(403, 'Forbidden');
    else { console.error(error); reply(500, 'Internal server error'); }
  }
});
server.on('error', (error) => { console.error(`Could not start the website: ${error.message}`); process.exitCode = 1; });
server.listen(port, '127.0.0.1', () => console.log(`Gateway website: http://127.0.0.1:${server.address().port}${values.base}\nServing website/dist. Rebuild with npm run build and refresh to see changes.`));
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => server.close());
