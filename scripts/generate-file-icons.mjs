import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const themeDir = resolve(root, "node_modules/material-icon-theme");
const iconsOutputDir = resolve(root, "public/file-icons");
const indexOutputPath = resolve(iconsOutputDir, "index.json");
const grammarsPath = resolve(root, "src/languages/generated/grammars.ts");
const check = process.argv.includes("--check");

const theme = JSON.parse(
  await readFile(resolve(themeDir, "dist/material-icons.json"), "utf8"),
);
const { version } = JSON.parse(
  await readFile(resolve(themeDir, "package.json"), "utf8"),
);

/** The app names a few languages differently from VS Code, whose names the theme uses. */
const LANGUAGE_NAMES = {
  docker: "dockerfile",
  jsx: "javascriptreact",
  make: "makefile",
  tsx: "typescriptreact",
};

const grammars = await readFile(grammarsPath, "utf8");
const appLanguages = new Set(
  [...grammars.matchAll(/^\s+"?([\w.+-]+)"?: \[/gm)].map((match) => match[1]),
);
for (const name of ["dotenv", "docker", "make"]) appLanguages.add(name);

const languages = {};
for (const language of [...appLanguages].sort()) {
  const icon =
    theme.languageIds[LANGUAGE_NAMES[language] ?? language] ??
    (theme.iconDefinitions[language] ? language : undefined);
  if (icon) languages[language] = icon;
}

const names = sorted(theme.fileNames);
const extensions = sorted(theme.fileExtensions);
const fallback = theme.file;

const used = new Set([
  fallback,
  ...Object.values(names),
  ...Object.values(extensions),
  ...Object.values(languages),
]);

const lightVariants = {};
for (const icon of used) {
  if (theme.iconDefinitions[`${icon}_light`])
    lightVariants[icon] = `${icon}_light`;
}

const icons = {};
for (const icon of [...used, ...Object.values(lightVariants)].sort()) {
  const definition = theme.iconDefinitions[icon];
  if (!definition) throw new Error(`The theme names a missing icon: ${icon}`);
  icons[icon] = await readFile(
    resolve(themeDir, "dist", definition.iconPath),
    "utf8",
  );
}

/** Every file name an icon is for, joined by spaces so the file stays small. */
function byIcon(record) {
  const grouped = {};
  for (const [name, icon] of Object.entries(record)) {
    if (/\s/.test(name)) throw new Error(`A file name holds a space: ${name}`);
    grouped[icon] = grouped[icon] ? `${grouped[icon]} ${name}` : name;
  }
  return sorted(grouped);
}

const indexOutput = `${JSON.stringify({
  version,
  file: fallback,
  names: byIcon(names),
  extensions: byIcon(extensions),
  languages,
  light: lightVariants,
})}\n`;

function sorted(record) {
  return Object.fromEntries(
    Object.entries(record).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
  );
}

async function readOrNull(path) {
  try {
    return await readFile(path, "utf8");
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
    return null;
  }
}

async function emit(path, expected) {
  if ((await readOrNull(path)) === expected) return;
  if (check) throw new Error(`Generated file icons are stale: ${path}`);
  await writeFile(path, expected);
}

await mkdir(iconsOutputDir, { recursive: true });
const expectedFiles = new Set([
  "index.json",
  "LICENSE",
  ...Object.keys(icons).map((icon) => `${icon}.svg`),
]);
for (const file of await readdir(iconsOutputDir)) {
  if (expectedFiles.has(file)) continue;
  if (check) throw new Error(`Generated file icons are stale: ${file}`);
  await rm(resolve(iconsOutputDir, file));
}
for (const [icon, svg] of Object.entries(icons))
  await emit(resolve(iconsOutputDir, `${icon}.svg`), svg);
await emit(indexOutputPath, indexOutput);
await emit(
  resolve(iconsOutputDir, "LICENSE"),
  await readFile(resolve(themeDir, "LICENSE"), "utf8"),
);
