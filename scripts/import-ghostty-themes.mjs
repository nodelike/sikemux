// Rebuilds src/themes/ghostty-themes.json from Ghostty's bundled theme files.
// Each value: background, foreground, cursor, cursor text, selection, and the 16 ANSI colours.
// Usage: node scripts/import-ghostty-themes.mjs [themes-dir]
// The themes come from iTerm2-Color-Schemes (MIT), which Ghostty ships as-is.
import { readdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const outputPath = resolve(root, "src/themes/ghostty-themes.json");
const themesDir =
  process.argv[2] ??
  "/Applications/Ghostty.app/Contents/Resources/ghostty/themes";

const KEYS = [
  "background",
  "foreground",
  "cursor-color",
  "cursor-text",
  "selection-background",
];

function hex(value) {
  const match = value.trim().match(/^#?([0-9a-f]{6}|[0-9a-f]{3})$/i);
  if (!match) return null;
  const digits = match[1].toLowerCase();
  return digits.length === 3
    ? digits
        .split("")
        .map((d) => d + d)
        .join("")
    : digits;
}

function parse(text) {
  const fields = {};
  const palette = [];
  for (const line of text.split("\n")) {
    const [key, ...rest] = line.split("=");
    const value = rest.join("=").trim();
    if (key.trim() === "palette") {
      const [index, color] = value.split("=");
      palette[Number(index)] = hex(color);
    } else if (KEYS.includes(key.trim())) fields[key.trim()] = hex(value);
  }
  if (!fields.background || !fields.foreground) return null;
  if (palette.slice(0, 16).filter(Boolean).length !== 16) return null;
  return [
    fields.background,
    fields.foreground,
    fields["cursor-color"] ?? fields.foreground,
    fields["cursor-text"] ?? fields.background,
    fields["selection-background"] ?? "",
    ...palette.slice(0, 16),
  ].join(",");
}

const names = (await readdir(themesDir))
  .filter((name) => !name.startsWith("."))
  .sort((a, b) => a.localeCompare(b));
const themes = {};
for (const name of names) {
  const encoded = parse(await readFile(join(themesDir, name), "utf8"));
  if (encoded) themes[name] = encoded;
}
const count = Object.keys(themes).length;
if (count === 0) throw new Error(`no Ghostty themes found in ${themesDir}`);

await writeFile(outputPath, `${JSON.stringify(themes, null, 2)}\n`);
console.log(`wrote ${count} themes to ${outputPath}`);
