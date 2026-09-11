import { readdir, readFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";

const assetDir = new URL("../dist/assets/", import.meta.url);
const files = await readdir(assetDir);
const requiredHeadroom = 0.1;

async function size(name) {
  const bytes = await readFile(new URL(name, assetDir));
  return { raw: bytes.byteLength, gzip: gzipSync(bytes).byteLength };
}

async function matching(pattern) {
  const names = files.filter((name) => pattern.test(name));
  const sizes = await Promise.all(names.map(size));
  return {
    names,
    raw: sizes.reduce((total, item) => total + item.raw, 0),
    gzip: sizes.reduce((total, item) => total + item.gzip, 0),
  };
}

const budgets = [
  {
    label: "startup JS",
    pattern: /^index-.*\.js$/,
    raw: 540_000,
    gzip: 165_000,
  },
  {
    label: "CodeMirror lazy chunk",
    pattern: /^codemirror-.*\.js$/,
    raw: 950_000,
    gzip: 340_000,
  },
  {
    label: "xterm core lazy chunk",
    pattern: /^xterm-(?!webgl).*\.js$/,
    raw: 450_000,
    gzip: 120_000,
  },
  {
    label: "ACP chat lazy chunk",
    pattern: /^AgentSurface-.*\.js$/,
    raw: 36_000,
    gzip: 12_000,
  },
  {
    label: "Diffs lazy chunk",
    pattern: /^diffs-.*\.js$/,
    raw: 2_320_000,
    gzip: 570_000,
  },
  {
    label: "Diffs worker chunks",
    pattern: /^(?:worker|wasm)-.*\.js$/,
    raw: 930_000,
    gzip: 335_000,
  },
  {
    label: "default-path JavaScript except Diffs",
    pattern: /^(?!(?:diffs|worker|wasm|paper-shaders|xterm-webgl)-).*\.js$/,
    raw: 2_820_000,
    gzip: 880_000,
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
    label: "ACP chat CSS",
    pattern: /^AgentSurface-.*\.css$/,
    raw: 24_000,
    gzip: 6_000,
  },
  {
    label: "application CSS",
    pattern: /^index-.*\.css$/,
    raw: 255_000,
    gzip: 44_000,
  },
  {
    label: "settings lazy CSS",
    pattern: /^SettingsPanel-.*\.css$/,
    raw: 40_000,
    gzip: 7_000,
  },
];

let failed = false;
for (const budget of budgets) {
  const actual = await matching(budget.pattern);
  if (actual.names.length === 0) {
    console.error(`performance budget: ${budget.label} chunk is missing`);
    failed = true;
    continue;
  }
  const rawHeadroom = 1 - actual.raw / budget.raw;
  const gzipHeadroom = 1 - actual.gzip / budget.gzip;
  const withinBudget =
    rawHeadroom >= requiredHeadroom && gzipHeadroom >= requiredHeadroom;
  const summary = `${budget.label}: raw ${actual.raw}/${budget.raw} (${(rawHeadroom * 100).toFixed(1)}% reserve), gzip ${actual.gzip}/${budget.gzip} (${(gzipHeadroom * 100).toFixed(1)}% reserve)`;
  if (withinBudget) console.log(`performance budget ok: ${summary}`);
  else {
    console.error(
      `performance budget exceeded: ${summary} (${actual.names.join(", ")})`,
    );
    failed = true;
  }
}

if (failed) process.exitCode = 1;
