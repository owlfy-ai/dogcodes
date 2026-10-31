import { createServer } from 'node:http';
import { readFileSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
const workspace = resolve(process.argv[2]);
const source = resolve(dirname(fileURLToPath(import.meta.url)), 'fixtures/computer-use-web');
mkdirSync(workspace, { recursive: true });
for (const file of ['index.html', 'app.js']) copyFileSync(resolve(source, file), resolve(workspace, file));
const title = `Dogcodes Web Acceptance ${process.argv[3]}`;
const server = createServer(async (request, response) => {
  if (request.url === '/report' && request.method === 'POST') {
    let body = '';
    for await (const chunk of request) { body += chunk; if (body.length > 4096) { response.writeHead(413).end(); return; } }
    try { const value = JSON.parse(body); if (!Number.isFinite(value.quantity) || !Number.isFinite(value.total)) throw Error('invalid'); writeFileSync(resolve(workspace, 'report.json'), JSON.stringify(value)); response.end('ok'); }
    catch { response.writeHead(400).end(); }
    return;
  }
  const file = request.url === '/' ? 'index.html' : request.url === '/app.js' ? 'app.js' : null;
  if (!file) { response.writeHead(404).end(); return; }
  response.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript; charset=utf-8' : 'text/html; charset=utf-8');
  response.setHeader('Cache-Control', 'no-store');
  response.end(readFileSync(resolve(workspace, file), 'utf8').replace('Dogcodes Web Acceptance</title>', `${title}</title>`));
});
server.listen(0, '127.0.0.1', () => writeFileSync(resolve(workspace, 'fixture.json'), JSON.stringify({ url: `http://127.0.0.1:${server.address().port}/`, title })));
