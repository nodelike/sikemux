import { readdir, readFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";

/*
 * Fails only on sizes a person would feel: startup code that delays the first
 * pane, a file large enough to stutter the pane that loads it, or styles heavy
 * enough to slow every layout. Ordinary feature growth fits under these.
 */

const distDir = new URL("../dist/", import.meta.url);
const assetDir = new URL("./assets/", distDir);
const files = await readdir(assetDir);

const STARTUP_JS = { raw: 2_000_000, gzip: 650_000 };
const LATER_FILE_RAW = 600_000;
const APP_CSS_RAW = 300_000;

const kb = (bytes) => `${Math.round(bytes / 1000)} KB`;
const failures = [];

// Mirrors the grammars src/vendor/shiki.ts bundles, so a build that folds them
// back into one chunk fails here instead of quietly growing another file.
const grammarChunks = [
  "css",
  "html",
  "json",
  "jsonc",
  "markdown",
  "python",
  "rust",
  "shellscript",
  "typescript",
  "yaml",
];

async function size(name) {
  const bytes = await readFile(new URL(name, assetDir));
  return { name, raw: bytes.byteLength, gzip: gzipSync(bytes).byteLength };
}

/** The entry chunk and everything it statically imports: what loads before a pane can draw. */
async function startupNames() {
  const html = await readFile(new URL("index.html", distDir), "utf8");
  const entry = html.match(/<script[^>]*\ssrc="\/assets\/([^"]+\.js)"/)?.[1];
  if (!entry) throw new Error("no entry script found in dist/index.html");
  const preloaded = [
    ...html.matchAll(/rel="modulepreload"[^>]*\shref="\/assets\/([^"]+\.js)"/g),
  ].map((match) => match[1]);

  const visited = new Set([entry]);
  const queue = [entry];
  while (queue.length > 0) {
    const content = await readFile(new URL(queue.pop(), assetDir), "utf8");
    const imports = [
      ...content.matchAll(/\bimport(?:[^"'();]*)from"\.\/([^"]+\.js)"/g),
      ...content.matchAll(/\bimport"\.\/([^"]+\.js)"/g),
    ].map((match) => match[1]);
    for (const name of imports) {
      if (visited.has(name)) continue;
      visited.add(name);
      queue.push(name);
    }
  }

  const late = [...visited].filter(
    (name) => name !== entry && !preloaded.includes(name),
  );
  if (late.length > 0) {
    failures.push(
      `imported at startup but not <link rel="modulepreload">, so the browser finds them late: ${late.join(", ")}`,
    );
  }
  return visited;
}

const startup = await startupNames();
const sizes = await Promise.all(
  files.filter((name) => /\.(?:js|css|wasm)$/.test(name)).map(size),
);

const startupSizes = sizes.filter(({ name }) => startup.has(name));
const startupRaw = startupSizes.reduce((sum, file) => sum + file.raw, 0);
const startupGzip = startupSizes.reduce((sum, file) => sum + file.gzip, 0);
console.log(
  `startup JS: ${kb(startupRaw)} of ${kb(STARTUP_JS.raw)}, gzip ${kb(startupGzip)} of ${kb(STARTUP_JS.gzip)} (${startupSizes.length} files)`,
);
if (startupRaw > STARTUP_JS.raw || startupGzip > STARTUP_JS.gzip) {
  const largest = [...startupSizes]
    .sort((a, b) => b.raw - a.raw)
    .slice(0, 5)
    .map((file) => `${file.name} ${kb(file.raw)}`);
  failures.push(
    `startup JS is over its limit; largest: ${largest.join(", ")}. Load what the first pane does not need with a dynamic import.`,
  );
}

const appCss = sizes.filter(({ name }) => /^index-.*\.css$/.test(name));
const appCssRaw = appCss.reduce((sum, file) => sum + file.raw, 0);
console.log(`application CSS: ${kb(appCssRaw)} of ${kb(APP_CSS_RAW)}`);
if (appCssRaw > APP_CSS_RAW) {
  failures.push(
    "application CSS is over its limit; move a feature's styles into the chunk that loads it",
  );
}

const later = sizes.filter(
  ({ name }) => !startup.has(name) && !/^index-.*\.css$/.test(name),
);
const largestLater = [...later].sort((a, b) => b.raw - a.raw)[0];
console.log(
  `largest file loaded later: ${largestLater.name} ${kb(largestLater.raw)} of ${kb(LATER_FILE_RAW)}`,
);
for (const file of later.filter((file) => file.raw > LATER_FILE_RAW)) {
  failures.push(
    `${file.name} is ${kb(file.raw)}, over ${kb(LATER_FILE_RAW)}; split it so opening its pane does not stutter`,
  );
}

for (const grammar of grammarChunks) {
  if (
    !files.some(
      (name) => name.startsWith(`${grammar}-`) && name.endsWith(".js"),
    )
  ) {
    failures.push(`the ${grammar} grammar has no chunk of its own`);
  }
}

for (const failure of failures) console.error(`performance budget: ${failure}`);
if (failures.length > 0) process.exitCode = 1;
