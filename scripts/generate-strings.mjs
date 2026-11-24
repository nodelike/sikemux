#!/usr/bin/env node
// Keeps the Mac app's string catalog, app/locales/Localizable.xcstrings, in step with the
// `t!("…")` and `n!("…")` calls in app/src and the commands' words in keybindings.json: a new
// string is added untranslated, one nothing uses any more is removed. Nothing checks it, since the
// build reads only the translations: run it before translating.
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const catalogPath = join(root, "app/locales/Localizable.xcstrings");

function* rustFiles(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* rustFiles(path);
    else if (name.endsWith(".rs")) yield path;
  }
}

const call = /\b[tn]!\(\s*"((?:[^"\\]|\\.)*)"/g;
const used = new Set();
const searchEntry =
  /\b(?:section|row)\(\s*\w+,\s*"((?:[^"\\]|\\.)*)",\s*(?:"((?:[^"\\]|\\.)*)",\s*)?"/g;
for (const file of rustFiles(join(root, "app/src"))) {
  const source = readFileSync(file, "utf8");
  for (const [, literal] of source.matchAll(call)) {
    used.add(JSON.parse(`"${literal}"`));
  }
  if (file.includes("/settings/")) {
    for (const [, first, second] of source.matchAll(searchEntry)) {
      for (const literal of [first, second])
        if (literal) used.add(JSON.parse(`"${literal}"`));
    }
  }
}

const shortcuts = JSON.parse(
  readFileSync(
    join(root, "crates/sikemux-core/src/assets/keybindings.json"),
    "utf8",
  ),
);
for (const shortcut of shortcuts) {
  for (const said of [shortcut.label, shortcut.detail, shortcut.category])
    if (said) used.add(said);
}

const checkOnly = process.argv.includes("--check");
const listMissing = process.argv.includes("--list");
const catalog = JSON.parse(readFileSync(catalogPath, "utf8"));
const languages = [
  ...new Set(
    Object.values(catalog.strings).flatMap((entry) =>
      Object.keys(entry.localizations ?? {}),
    ),
  ),
].sort();

const untranslated = [];
for (const key of [...used].sort()) {
  const localizations = catalog.strings[key]?.localizations ?? {};
  if (
    languages.some((language) => !localizations[language]?.stringUnit?.value)
  ) {
    untranslated.push(key);
  }
}

if (!checkOnly && !listMissing) {
  const strings = {};
  for (const key of [...used].sort()) strings[key] = catalog.strings[key] ?? {};
  const next = `${JSON.stringify({ ...catalog, strings }, null, 2)}\n`;
  if (next !== readFileSync(catalogPath, "utf8"))
    writeFileSync(catalogPath, next);
}

if (listMissing) {
  for (const key of untranslated) console.log(key);
}
console.log(
  `✓ strings: ${used.size} strings, ${untranslated.length} untranslated`,
);
