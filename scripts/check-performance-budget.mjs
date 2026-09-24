import { readdir, readFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";

const distDir = new URL("../dist/", import.meta.url);
const assetDir = new URL("./assets/", distDir);
const files = await readdir(assetDir);
const requiredHeadroom = 0.1;

async function size(name) {
  const bytes = await readFile(new URL(name, assetDir));
  return { raw: bytes.byteLength, gzip: gzipSync(bytes).byteLength };
}

async function matching(pattern) {
  const names = files.filter((name) => pattern.test(name));
  return sizeOfNames(names);
}

async function sizeOfNames(names) {
  const sizes = await Promise.all(names.map(size));
  return {
    names,
    raw: sizes.reduce((total, item) => total + item.raw, 0),
    gzip: sizes.reduce((total, item) => total + item.gzip, 0),
  };
}

// The entry's own eager set: everything the browser fetches before the app
// can render a single pane, i.e. the entry chunk plus every JS file it (or
// something it statically imports) pulls in via `import ... from`. This
// walks the real import graph instead of trusting a single filename pattern,
// so it stays correct if a chunk that used to be lazy becomes part of the
// boot path (or the reverse) without anyone remembering to update this file.
async function computeEagerJsSet() {
  const html = await readFile(new URL("index.html", distDir), "utf8");
  const entryMatch = html.match(/<script[^>]*\ssrc="\/assets\/([^"]+\.js)"/);
  if (!entryMatch)
    throw new Error(
      "performance budget: no entry script found in dist/index.html",
    );
  const entry = entryMatch[1];

  const preloaded = [
    ...html.matchAll(/rel="modulepreload"[^>]*\shref="\/assets\/([^"]+\.js)"/g),
  ].map((m) => m[1]);

  const visited = new Set([entry]);
  const queue = [entry];
  while (queue.length > 0) {
    const name = queue.pop();
    const content = await readFile(new URL(name, assetDir), "utf8");
    const staticImports = [
      ...content.matchAll(/\bimport(?:[^"'();]*)from"\.\/([^"]+\.js)"/g),
      ...content.matchAll(/\bimport"\.\/([^"]+\.js)"/g),
    ].map((m) => m[1]);
    for (const dep of staticImports) {
      if (!visited.has(dep)) {
        visited.add(dep);
        queue.push(dep);
      }
    }
  }

  const missingPreloads = [...visited].filter(
    (name) => name !== entry && !preloaded.includes(name),
  );
  if (missingPreloads.length > 0) {
    throw new Error(
      `performance budget: statically imported by the entry but not <link rel="modulepreload">, so the browser discovers them late: ${missingPreloads.join(", ")}`,
    );
  }

  return [...visited];
}

// Grammar chunks load one at a time, on demand, keyed by the language of
// the file being diffed or of the code fence being coloured in a chat
// (src/vendor/shiki.ts's bundledLanguages map). This
// list mirrors those keys (minus "zsh", which shares the "shellscript"
// loader) so a regression that re-folds them into one big chunk shows up as
// a missing-chunk failure below instead of silently vanishing into the
// "default-path JavaScript" catch-all.
const diffLanguageChunkNames = [
  "c",
  "css",
  "go",
  "html",
  "java",
  "json",
  "jsonc",
  "markdown",
  "python",
  "rust",
  "shellscript",
  "sql",
  "typescript",
  "yaml",
];
const diffLanguageChunkPattern = new RegExp(
  `^(?:${diffLanguageChunkNames.join("|")})-.*\\.js$`,
);

const eagerJsSet = await computeEagerJsSet();
const startupJs = await sizeOfNames(eagerJsSet);

const budgets = [
  {
    label:
      "startup JS (index + every modulepreloaded/statically-imported chunk)",
    actual: startupJs,
    raw: 2_400_000,
    gzip: 760_000,
  },
  {
    label: "CodeMirror core chunk",
    pattern: /^codemirror-core-.*\.js$/,
    raw: 470_000,
    gzip: 150_000,
  },
  {
    label: "CodeMirror language-pack chunk",
    pattern: /^codemirror-langs-.*\.js$/,
    raw: 475_000,
    gzip: 180_000,
  },
  {
    label: "xterm core chunk",
    pattern: /^xterm-(?!webgl).*\.js$/,
    raw: 450_000,
    gzip: 120_000,
  },
  {
    // Carries the part of colouring code that has to be there before any
    // colours are: the grammar table, the token cache, the tokens' own markup
    // and the splitting a diff line's changed span needs. Shiki itself is a
    // chunk of its own, fetched only once a block that can use it settles.
    label: "ACP chat lazy chunk",
    pattern: /^AgentSurface-.*\.js$/,
    raw: 82_000,
    gzip: 26_700,
  },
  {
    // Shiki, its JavaScript regex engine and vscode-textmate, with no
    // grammars and no themes (both are stubbed or dynamic). Fetched on demand the first time
    // a diff is opened or a chat fence with a grammar we have settles, and
    // shared by both from then on.
    label: "Highlighter lazy chunk (shiki core + JS engine, no grammars)",
    pattern: /^highlighter-.*\.js$/,
    raw: 680_000,
    gzip: 190_000,
  },
  {
    label: "Diffs lazy chunk (pierre/diffs, no highlighter)",
    pattern: /^diffs-.*\.js$/,
    raw: 60_000,
    gzip: 20_000,
  },
  {
    label: "Diffs language grammar chunks (one per language, loaded on demand)",
    pattern: diffLanguageChunkPattern,
    raw: 950_000,
    gzip: 130_000,
  },
  {
    label: "Diffs worker chunks",
    pattern: /^(?:worker|wasm)-.*\.js$/,
    raw: 930_000,
    gzip: 335_000,
  },
  {
    label: "default-path JavaScript except Diffs and its grammar chunks",
    pattern: new RegExp(
      `^(?!(?:diffs|highlighter|worker|wasm|paper-shaders|xterm-webgl|${diffLanguageChunkNames.join("|")})-).*\\.js$`,
    ),
    raw: 3_120_000,
    gzip: 1_000_000,
  },
  {
    label: "opt-in shader renderer",
    pattern: /^paper-shaders-.*\.js$/,
    raw: 70_000,
    gzip: 35_000,
  },
  {
    label: "opt-in xterm WebGL renderer",
    pattern: /^xterm-webgl-.*\.js$/,
    raw: 130_000,
    gzip: 36_000,
  },
  {
    // The chat pane has since grown rows the budget predates: subagent
    // transcripts, background tasks, queued messages and the reconnect
    // states. It is one lazily loaded sheet behind an agent pane, so this
    // buys those rows room without touching what the app loads at startup.
    label: "ACP chat CSS",
    pattern: /^AgentSurface-.*\.css$/,
    raw: 39_000,
    gzip: 7_300,
  },
  {
    // Includes the JetBrainsMono Nerd Font @font-face rules: a base face per
    // weight/style plus an icons face, each carrying an explicit
    // unicode-range so the ~930 KB icons file only downloads once a PUA
    // glyph is actually rendered. Plugin panes bring their own sheets.
    label: "application CSS",
    pattern: /^index-.*\.css$/,
    raw: 216_000,
    gzip: 37_800,
  },
  {
    label: "settings lazy CSS",
    pattern: /^SettingsPanel-.*\.css$/,
    raw: 42_300,
    gzip: 7_000,
  },
];

// Rather than a hand-written "this chunk is eager today" note that goes
// stale the moment someone else's change makes it lazy (or vice versa),
// compare each JS budget's matched files against the measured eager set and
// report reality every run.
function eagerness(actual) {
  if (actual.names.length === 0) return "";
  const jsNames = actual.names.filter((name) => name.endsWith(".js"));
  if (jsNames.length === 0) return "";
  const eagerCount = jsNames.filter((name) => eagerJsSet.includes(name)).length;
  if (eagerCount === jsNames.length) return " [part of startup JS]";
  if (eagerCount === 0) return " [lazy]";
  return " [partly in startup JS]";
}

let failed = false;
for (const budget of budgets) {
  const actual = budget.actual ?? (await matching(budget.pattern));
  if (actual.names.length === 0) {
    console.error(`performance budget: ${budget.label} chunk is missing`);
    failed = true;
    continue;
  }
  const rawHeadroom = 1 - actual.raw / budget.raw;
  const gzipHeadroom = 1 - actual.gzip / budget.gzip;
  const withinBudget =
    rawHeadroom >= requiredHeadroom && gzipHeadroom >= requiredHeadroom;
  const summary = `${budget.label}${eagerness(actual)}: raw ${actual.raw}/${budget.raw} (${(rawHeadroom * 100).toFixed(1)}% reserve), gzip ${actual.gzip}/${budget.gzip} (${(gzipHeadroom * 100).toFixed(1)}% reserve)`;
  if (withinBudget) console.log(`performance budget ok: ${summary}`);
  else {
    console.error(
      `performance budget exceeded: ${summary} (${actual.names.join(", ")})`,
    );
    failed = true;
  }
}

if (failed) process.exitCode = 1;
