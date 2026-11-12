#!/usr/bin/env node
// Keeps the Mac app's string catalog, app/locales/Localizable.xcstrings, in step with the
// `t!("…")` and `n!("…")` calls in app/src and the commands' words in keybindings.json: a new
// string is added untranslated, one nothing uses any more is removed.
// With --check it only says whether the catalog is stale.
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const catalogPath = join(root, "app/locales/Localizable.xcstrings");
const check = process.argv.includes("--check");
const languages = ["es", "zh-Hans", "de", "fr", "ja", "ko", "ru", "pt-BR"];

function* rustFiles(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* rustFiles(path);
    else if (name.endsWith(".rs")) yield path;
  }
}

const call = /\b[tn]!\(\s*"((?:[^"\\]|\\.)*)"/g;
const used = new Set();
for (const file of rustFiles(join(root, "app/src"))) {
  for (const [, literal] of readFileSync(file, "utf8").matchAll(call)) {
    used.add(JSON.parse(`"${literal}"`));
  }
}

const shortcuts = JSON.parse(
  readFileSync(join(root, "crates/sikemux-core/src/assets/keybindings.json"), "utf8"),
);
for (const shortcut of shortcuts) {
  for (const said of [shortcut.label, shortcut.detail, shortcut.category]) if (said) used.add(said);
}

const catalog = JSON.parse(readFileSync(catalogPath, "utf8"));
const strings = {};
for (const key of [...used].sort()) strings[key] = catalog.strings[key] ?? {};
const next = `${JSON.stringify({ ...catalog, strings }, null, 2)}\n`;
const stale = next !== readFileSync(catalogPath, "utf8");

if (check) {
  if (stale) {
    console.error("✗ strings: app/locales/Localizable.xcstrings is stale; run pnpm strings:generate");
    process.exit(1);
  }
  const missing = [...used].filter(
    (key) => languages.some((language) => !strings[key].localizations?.[language]),
  );
  console.log(`✓ strings: ${used.size} strings, ${missing.length} not in every language`);
} else {
  if (stale) writeFileSync(catalogPath, next);
  console.log(`✓ strings: ${used.size} strings`);
}
