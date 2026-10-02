// Builds screens.html from screens.src.html: `{{IconName}}` or `{{IconName:size}}`
// becomes that icon's SVG, the same markup the phone app draws. `--serve` also
// serves the Mac app's fonts from public/fonts, which the page loads.
import { createReadStream, existsSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { dirname, extname, join, normalize, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

import { generate } from '../app/scripts/generate.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '../..');
const app = resolve(here, '../app');
const PORT = 8791;
const SERVED = [here, join(repo, 'public/fonts')];

await generate();
const text = readFileSync(resolve(app, 'src/ui/icons.generated.ts'), 'utf8');
const icons = JSON.parse(text.slice(text.indexOf('{'), text.lastIndexOf('} as const') + 1));

const { build: bundle } = createRequire(resolve(app, 'package.json'))('esbuild');
const drawnModule = await bundle({ entryPoints: [resolve(app, 'src/ui/drawnIcons.ts')], format: 'esm', write: false });
const { DRAWN_ICONS } = await import(`data:text/javascript,${encodeURIComponent(drawnModule.outputFiles[0].text)}`);
for (const [name, paths] of Object.entries(DRAWN_ICONS)) {
  icons[`Icon${name[0].toUpperCase()}${name.slice(1)}`] =
    `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" ` +
    `stroke-linecap="round" stroke-linejoin="round">${paths}</svg>`;
}

function icon(_, name, size = '16') {
  const svg = icons[name];
  if (!svg) throw new Error(`screens.src.html names an icon the app does not have: ${name}`);
  return svg
    .replace(/width="\d+"/, `width="${size}"`)
    .replace(/height="\d+"/, `height="${size}"`)
    .replace('<svg ', '<svg class="ico" aria-hidden="true" ');
}

function build() {
  const source = readFileSync(resolve(here, 'screens.src.html'), 'utf8');
  writeFileSync(resolve(here, 'screens.html'), source.replace(/\{\{(Icon\w+|Logo)(?::(\d+))?\}\}/g, icon));
}

build();
if (!process.argv.includes('--serve')) {
  console.log('Built mobile/design/screens.html');
  process.exit(0);
}

const types = { '.html': 'text/html', '.woff2': 'font/woff2', '.svg': 'image/svg+xml', '.png': 'image/png', '.css': 'text/css' };
createServer((request, response) => {
  const path = decodeURIComponent(new URL(request.url ?? '/', 'http://localhost').pathname);
  // Each visit to the page rebuilds it, so an edit shows on reload.
  if (path === '/mobile/design/screens.html') build();
  const file = normalize(join(repo, path));
  const allowed = SERVED.some((dir) => file.startsWith(dir + sep));
  if (!allowed || !existsSync(file) || !statSync(file).isFile()) {
    response.writeHead(404).end();
    return;
  }
  response.writeHead(200, { 'content-type': types[extname(file)] ?? 'application/octet-stream' });
  createReadStream(file).pipe(response);
}).listen(PORT, '127.0.0.1', () => {
  console.log(`Phone screens: http://127.0.0.1:${PORT}/mobile/design/screens.html`);
});
