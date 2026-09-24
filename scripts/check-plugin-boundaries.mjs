// A plugin reaches the app only through src/plugin-api, and the app reaches a
// plugin only through the list of built-ins. This resolves every import to
// where it actually lands, which a pattern over relative paths cannot do.
import { readdir, readFile } from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";

const root = resolve(import.meta.dirname, "..");
const src = join(root, "src");
const pluginsDir = join(src, "plugins");
const pluginApi = join(src, "plugin-api");
const builtins = join(pluginsDir, "builtin.ts");
const PLUGIN_PACKAGES = [
  /^react(\/|$)/u,
  /^zustand(\/|$)/u,
  /^vitest$/u,
  /^@testing-library\//u,
];
const IMPORT =
  /(?:^|\n)\s*(?:import|export)\s[^;]*?from\s+["']([^"']+)["']|(?:^|\n)\s*import\s+["']([^"']+)["']|import\(\s*["']([^"']+)["']\s*\)/gu;

async function sourceFiles(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map((entry) => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return sourceFiles(path);
      return /\.(ts|tsx)$/u.test(entry.name) ? [path] : [];
    }),
  );
  return nested.flat();
}

const inside = (path, dir) => path === dir || path.startsWith(dir + sep);
const isTest = (file) => /\.test\.tsx?$/u.test(file);

function pluginFolderOf(file) {
  if (!inside(file, pluginsDir)) return null;
  const [first, ...rest] = relative(pluginsDir, file).split(sep);
  return rest.length > 0 ? join(pluginsDir, first) : null;
}

const problems = [];
for (const file of await sourceFiles(src)) {
  const text = await readFile(file, "utf8");
  const ownPlugin = pluginFolderOf(file);
  for (const match of text.matchAll(IMPORT)) {
    const specifier = match[1] ?? match[2] ?? match[3];
    const where = relative(root, file);
    if (!specifier.startsWith(".")) {
      if (
        ownPlugin &&
        !PLUGIN_PACKAGES.some((allowed) => allowed.test(specifier))
      )
        problems.push(
          `${where}: a plugin imports the package "${specifier}"; reach it through src/plugin-api`,
        );
      continue;
    }
    const target = resolve(dirname(file), specifier);
    const targetPlugin = pluginFolderOf(target);
    if (ownPlugin) {
      if (!inside(target, ownPlugin) && !inside(target, pluginApi))
        problems.push(
          `${where}: a plugin imports "${specifier}" from outside itself; add what it needs to src/plugin-api`,
        );
    } else if (targetPlugin && file !== builtins && !isTest(file)) {
      problems.push(
        `${where}: core imports the plugin at "${specifier}"; only src/plugins/builtin.ts may`,
      );
    }
  }
}

if (problems.length > 0) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log("✓ plugins and core meet only at src/plugin-api");
