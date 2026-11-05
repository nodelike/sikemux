// What the generators do alike: `--check` fails on an output that is out of date
// instead of writing it.
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { createServer } from "vite";

export const check = process.argv.includes("--check");
export const root = resolve(import.meta.dirname, "..", "..");

export function stale(path) {
  return new Error(`Generated file is out of date: ${path}; run its generator`);
}

export async function emit(path, contents) {
  const actual = await readFile(path, "utf8").catch((error) => {
    if (error?.code !== "ENOENT") throw error;
    return null;
  });
  if (actual === contents) return;
  if (check) throw stale(path);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, contents);
}

/** A Vite server for loading the shared TypeScript a generator reads. */
export function typescriptLoader() {
  return createServer({
    root,
    logLevel: "error",
    server: { middlewareMode: true, hmr: false },
    appType: "custom",
    optimizeDeps: { noDiscovery: true, include: [] },
    resolve: {
      alias: [
        { find: /^@shared\//, replacement: `${resolve(root, "shared")}/` },
      ],
    },
  });
}

const GRAMMAR =
  /^const lang = Object\.freeze\(JSON\.parse\(("(?:[^"\\]|\\.)*")\)\)$/m;
const IMPORT = /^import \w+ from '\.\/([\w.+-]+)\.mjs'$/gm;
const ALIAS = /^export \{ default \} from '\.\/([\w.+-]+)\.mjs'$/m;

/**
 * Shiki's grammars: each one's text, the grammars it embeds, and the names
 * that only point at another.
 */
export async function shikiGrammars(langsDir) {
  const sources = {};
  const embeds = {};
  const aliases = {};
  for (const file of (await readdir(resolve(langsDir, "dist"))).sort()) {
    if (!file.endsWith(".mjs") || file === "index.mjs") continue;
    const id = file.slice(0, -".mjs".length);
    const module = await readFile(resolve(langsDir, "dist", file), "utf8");
    const alias = ALIAS.exec(module);
    if (alias) {
      aliases[id] = alias[1];
      continue;
    }
    const literal = GRAMMAR.exec(module);
    if (!literal) throw new Error(`Unrecognised grammar module: ${file}`);
    sources[id] = JSON.parse(literal[1]);
    embeds[id] = [...module.matchAll(IMPORT)].map((match) => match[1]);
  }
  return { sources, embeds, aliases };
}

/** Languages Shiki and the icon theme lack, kept in `shared/languages`, each with its grammar. */
export async function localLanguages() {
  const dir = resolve(root, "shared/languages");
  const found = {};
  for (const file of (await readdir(dir)).sort()) {
    if (!file.endsWith(".tmLanguage.json")) continue;
    const id = file.slice(0, -".tmLanguage.json".length);
    found[id] = { grammar: await readFile(resolve(dir, file), "utf8"), dir };
  }
  return found;
}
