import { readdir, readFile } from "node:fs/promises";
import { Linter } from "eslint";
import globals from "globals";

const assetDir = new URL("../dist/assets/", import.meta.url);
const files = (await readdir(assetDir))
  .filter((name) => name.endsWith(".js"))
  .sort();

if (files.length === 0) {
  throw new Error("bundle globals: no JavaScript assets found");
}

const linter = new Linter({ configType: "flat" });
const languageGlobals = {
  ...globals.browser,
  // React and UMD wrappers use guarded references to these optional
  // host globals.
  __REACT_DEVTOOLS_GLOBAL_HOOK__: "readonly",
  define: "readonly",
  process: "readonly",
  setImmediate: "readonly",
};
const failures = [];
const chunkImports = new Map();

for (const name of files) {
  const source = await readFile(new URL(name, assetDir), "utf8");
  const imports = new Set();
  const collectSource = (node) => {
    const value = node.source?.value;
    if (typeof value === "string" && value.startsWith("./")) {
      imports.add(value.slice(2));
    }
  };
  const messages = linter.verify(
    source,
    [
      {
        plugins: {
          bundle: {
            rules: {
              "collect-imports": {
                meta: { schema: [] },
                create: () => ({
                  ImportDeclaration: collectSource,
                  ExportNamedDeclaration: collectSource,
                  ExportAllDeclaration: collectSource,
                }),
              },
            },
          },
        },
        languageOptions: {
          ecmaVersion: "latest",
          sourceType: "module",
          globals: languageGlobals,
        },
        rules: {
          "bundle/collect-imports": "error",
          "no-undef": "error",
        },
      },
    ],
    { filename: name },
  );

  for (const message of messages) {
    if (message.ruleId !== "no-undef") continue;
    failures.push(
      `${name}:${message.line}:${message.column} ${message.message}`,
    );
  }
  chunkImports.set(name, imports);
}

function findImportCycle() {
  const visited = new Set();
  const active = new Set();
  const path = [];

  const visit = (name) => {
    if (active.has(name)) {
      return [...path.slice(path.indexOf(name)), name];
    }
    if (visited.has(name)) return null;
    active.add(name);
    path.push(name);
    for (const dependency of chunkImports.get(name) ?? []) {
      if (!chunkImports.has(dependency)) continue;
      const cycle = visit(dependency);
      if (cycle) return cycle;
    }
    path.pop();
    active.delete(name);
    visited.add(name);
    return null;
  };

  for (const name of files) {
    const cycle = visit(name);
    if (cycle) return cycle;
  }
  return null;
}

const importCycle = findImportCycle();
if (importCycle) {
  failures.push(`circular static chunk imports: ${importCycle.join(" -> ")}`);
}

if (failures.length > 0) {
  throw new Error(`bundle globals:\n${failures.join("\n")}`);
}

console.log(`bundle globals ok: ${files.length} JavaScript assets checked`);
