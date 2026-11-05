import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { gunzipSync, gzipSync } from "node:zlib";
import {
  check,
  emit,
  localLanguages,
  stale,
  typescriptLoader,
} from "./lib/generated.mjs";

const root = resolve(import.meta.dirname, "..");
const themeDir = resolve(root, "node_modules/material-icon-theme");
const iconsOutputDir = resolve(root, "server/app/public/file-icons");
const indexOutputPath = resolve(iconsOutputDir, "index.json");
const gpuiBundlePath = resolve(root, "app/assets/file-icons.json.gz");
const grammarsPath = resolve(root, "shared/languages/grammars.generated.ts");

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

const server = await typescriptLoader();
const { FILE_NAMES, EXTENSIONS } = await server.ssrLoadModule(
  "/shared/languages/index.ts",
);
const { GRAMMARS, GRAMMAR_ALIASES } = await server.ssrLoadModule(
  "/shared/languages/grammars.generated.ts",
);
const appLanguages = new Set(Object.keys(GRAMMARS));
for (const name of ["dotenv", "docker", "make"]) appLanguages.add(name);

// Languages Shiki and the icon theme lack, each a grammar and an icon of its own.
const ownLanguages = {};
for (const [id, { grammar, dir }] of Object.entries(await localLanguages())) {
  ownLanguages[id] = {
    extensions: JSON.parse(grammar).fileTypes ?? [],
    icon: await readFile(resolve(dir, `${id}.svg`), "utf8"),
  };
}

const languages = {};
for (const language of [...appLanguages].sort()) {
  const icon =
    theme.languageIds[LANGUAGE_NAMES[language] ?? language] ??
    (theme.iconDefinitions[language] ? language : undefined);
  if (icon) languages[language] = icon;
}

for (const id of Object.keys(ownLanguages)) languages[id] = id;

const names = sorted(theme.fileNames);
const extensions = sorted({
  ...theme.fileExtensions,
  ...Object.fromEntries(
    Object.entries(ownLanguages).flatMap(([id, { extensions }]) =>
      extensions.map((extension) => [extension, id]),
    ),
  ),
});
const fallback = theme.file;

const used = new Set(
  [
    fallback,
    ...Object.values(names),
    ...Object.values(extensions),
    ...Object.values(languages),
  ].filter((icon) => !ownLanguages[icon]),
);

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

for (const [id, { icon }] of Object.entries(ownLanguages)) icons[id] = icon;

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

/** Compressed files are compared by what they hold, since zlib versions differ in the bytes they write. */
async function emitPacked(path, text) {
  const current = await readFile(path).catch(() => null);
  if (current && gunzipSync(current).toString("utf8") === text) return;
  if (check) throw stale(path);
  await writeFile(path, gzipSync(text, { level: 9 }));
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

// The GPUI app reads one compressed bundle: the same index and icons, with the
// tables the app tells a file's language by, so both pick the same icon.
try {
  const known = sorted({
    ...Object.fromEntries(
      [...Object.keys(GRAMMARS), ...Object.keys(ownLanguages)].map((id) => [
        id,
        id,
      ]),
    ),
    ...GRAMMAR_ALIASES,
  });
  await emitPacked(
    gpuiBundlePath,
    JSON.stringify({
      version,
      file: fallback,
      names,
      extensions,
      languages,
      light: lightVariants,
      languageNames: sorted(FILE_NAMES),
      languageExtensions: sorted(EXTENSIONS),
      known,
      icons,
    }),
  );
} finally {
  await server.close();
}
