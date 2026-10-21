#!/usr/bin/env node
// Fails when a plugin's two sides disagree. Its TypeScript, and each of its
// agent tools, may ask only for methods its Rust side serves: the string arms
// of each `match method {` in lib.rs and in the modules lib.rs hands `method`
// to. TypeScript asks with a literal in the `method` parameter of any function
// that takes one, such as a backend's `call` and `stream`. And a plugin whose
// Rust names its service, in error.rs, names it as its manifest does.

import { readdir, readFile, stat } from "node:fs/promises";
import { join, relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const RUST_PLUGINS = join(root, "src-tauri/plugins");
const TS_PLUGINS = join(root, "src/plugins");
const SHARED = [
  join(root, "src/plugin-api"),
  join(root, "src/codehost/client.ts"),
];

const where = (file) => relative(root, file);
const exists = (path) =>
  stat(path).then(
    () => true,
    () => false,
  );

async function sourceFiles(path) {
  if (!(await stat(path)).isDirectory()) return [path];
  const entries = await readdir(path, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map((entry) => sourceFiles(join(path, entry.name))),
  );
  return nested
    .flat()
    .filter(
      (file) =>
        /\.tsx?$/u.test(file) &&
        !/\.test\.tsx?$|fakeTransport\.ts$/u.test(file),
    );
}

const OPEN = "([{";
const CLOSE = ")]}";

/** Where the string literal opening at `start` ends, just past its closing quote. */
function stringEnd(text, start) {
  const quote = text[start];
  let at = start + 1;
  while (at < text.length && text[at] !== quote)
    at += text[at] === "\\" ? 2 : 1;
  return at + 1;
}

/** The method names a Rust `match method {` block opening at `brace` matches on. */
function matchArms(text, brace) {
  const arms = [];
  let depth = 0;
  for (let at = brace; at < text.length; at++) {
    const char = text[at];
    if (text.startsWith("//", at)) at = text.indexOf("\n", at);
    else if (char === '"') {
      const end = stringEnd(text, at);
      const after = text.slice(end).trimStart();
      if (depth === 1 && /^(=>|\||if\s)/u.test(after))
        arms.push(text.slice(at + 1, end - 1));
      at = end - 1;
    } else if (OPEN.includes(char)) depth++;
    else if (CLOSE.includes(char) && --depth === 0) return arms;
  }
  throw new Error(`a \`match method\` block never closes`);
}

/** Every method a Rust plugin serves, by the files that serve it. */
async function served(plugin) {
  const src = join(RUST_PLUGINS, plugin, "src");
  const lib = await readFile(join(src, "lib.rs"), "utf8");
  const files = [join(src, "lib.rs")];
  for (const [, module, args] of lib.matchAll(
    /\b(\w+)::\w+\s*\(([^()]*)\)/gu,
  )) {
    if (!args.split(",").some((arg) => arg.trim() === "method")) continue;
    for (const file of [
      join(src, `${module}.rs`),
      join(src, module, "mod.rs"),
    ]) {
      if (!files.includes(file) && (await exists(file))) files.push(file);
    }
  }
  const methods = new Set();
  for (const file of files) {
    const text = await readFile(file, "utf8");
    for (const match of text.matchAll(/\bmatch method \{/gu)) {
      for (const arm of matchArms(text, match.index + match[0].length - 1))
        methods.add(arm);
    }
  }
  return methods;
}

/** The arguments of the call whose parenthesis opens at `paren`, as source text. */
function callArguments(text, paren) {
  const args = [];
  let depth = 0;
  let from = paren + 1;
  for (let at = paren; at < text.length; at++) {
    const char = text[at];
    if (char === '"' || char === "'" || char === "`")
      at = stringEnd(text, at) - 1;
    else if (OPEN.includes(char)) depth++;
    else if (CLOSE.includes(char) && --depth === 0)
      return [...args, text.slice(from, at)].map((arg) => arg.trim());
    else if (char === "," && depth === 1) {
      args.push(text.slice(from, at));
      from = at + 1;
    }
  }
  return args;
}

/** Past the type arguments, if any, that start at `at`. */
function skipTypeArguments(text, at) {
  if (text[at] !== "<") return at;
  let depth = 0;
  for (; at < text.length; at++) {
    if (text[at] === "<") depth++;
    else if (text[at] === ">" && text[at - 1] !== "=" && --depth === 0)
      return at + 1;
  }
  return at;
}

/** Each function that takes a `method: string`, with where in its parameters it takes it. */
function methodTakers(files) {
  const takers = new Map();
  for (const { file, text } of files) {
    for (const match of text.matchAll(/\bmethod: string\b/gu)) {
      let depth = 0;
      let commas = 0;
      let at = match.index - 1;
      for (; at >= 0; at--) {
        const char = text[at];
        if (CLOSE.includes(char) || (char === ">" && text[at - 1] !== "="))
          depth++;
        else if (OPEN.includes(char) || char === "<") {
          if (depth === 0) break;
          depth--;
        } else if (char === "," && depth === 0) commas++;
      }
      if (text[at] !== "(") continue;
      const before = text
        .slice(0, at)
        .replace(/<[^<>]*(?:<[^<>]*>[^<>]*)*>\s*$/u, "");
      const name = /(\w+)\s*(?:[:=]\s*(?:async\s*)?)?$/u.exec(before)?.[1];
      if (!name || name === "function") continue;
      const known = takers.get(name);
      if (known !== undefined && known !== commas) {
        throw new Error(
          `${where(file)}: \`${name}\` takes its method as argument ${commas + 1}, and elsewhere as argument ${known + 1}`,
        );
      }
      takers.set(name, commas);
    }
  }
  return takers;
}

/** Each literal method a file asks for, with its line. */
function asked(text, takers) {
  const found = [];
  for (const [name, index] of takers) {
    for (const match of text.matchAll(new RegExp(`\\b${name}\\b`, "gu"))) {
      const paren = skipTypeArguments(text, match.index + name.length);
      if (text[paren] !== "(") continue;
      const literal = /^(["'`])(\w+)\1$/u.exec(
        callArguments(text, paren)[index] ?? "",
      );
      if (literal)
        found.push({
          method: literal[2],
          at: match.index,
          line: text.slice(0, match.index).split("\n").length,
        });
    }
  }
  return found;
}

/** A shared file's top-level declarations, each with the text it spans. */
function declarations(text) {
  const starts = [
    ...text.matchAll(
      /^(?:export\s+)?(?:async\s+)?(?:function\s+|const\s+|class\s+)(\w+)/gmu,
    ),
  ];
  return starts.map((start, index) => ({
    name: start[1],
    from: start.index,
    to: starts[index + 1]?.index ?? text.length,
  }));
}

const read = async (paths) =>
  Promise.all(
    (await Promise.all(paths.map(sourceFiles)))
      .flat()
      .map(async (file) => ({ file, text: await readFile(file, "utf8") })),
  );

const plugins = (await readdir(TS_PLUGINS, { withFileTypes: true }))
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name);
const shared = await read(SHARED);
const pluginFiles = new Map(
  await Promise.all(
    plugins.map(async (plugin) => [
      plugin,
      await read([join(TS_PLUGINS, plugin)]),
    ]),
  ),
);
const takers = methodTakers([shared, ...pluginFiles.values()].flat());

const sharedDeclarations = shared.flatMap(({ file, text }) => {
  const calls = asked(text, takers);
  return declarations(text).map((declaration) => ({
    ...declaration,
    file,
    body: text.slice(declaration.from, declaration.to),
    asks: calls
      .filter((call) => call.at >= declaration.from && call.at < declaration.to)
      .map((call) => ({ ...call, file })),
  }));
});

/** What the shared declarations a text names ask for, following the ones they name in turn. */
function sharedAsks(text) {
  const seen = new Set();
  const pending = [text];
  const asks = [];
  while (pending.length > 0) {
    const next = pending.pop();
    for (const declaration of sharedDeclarations) {
      if (
        seen.has(declaration) ||
        !new RegExp(`\\b${declaration.name}\\b`, "u").test(next)
      )
        continue;
      seen.add(declaration);
      asks.push(...declaration.asks);
      pending.push(declaration.body);
    }
  }
  return asks;
}

/** The id and name a plugin's Rust gives its service in error.rs, or null when it gives none. */
async function serviceIdentity(plugin) {
  const file = join(RUST_PLUGINS, plugin, "src/error.rs");
  if (!(await exists(file))) return null;
  const impl = /\bimpl Service for \w+ \{([\s\S]*?)\n\}/u.exec(
    await readFile(file, "utf8"),
  );
  if (!impl) return null;
  const constant = (name) =>
    new RegExp(`\\bconst ${name}: &'static str = "([^"]*)";`, "u").exec(
      impl[1],
    )?.[1];
  return { file, id: constant("ID"), name: constant("NAME") };
}

const problems = [];
for (const plugin of plugins) {
  const rust = join(RUST_PLUGINS, plugin);
  if (!(await exists(join(rust, "src/lib.rs")))) {
    problems.push(
      `src/plugins/${plugin} has no Rust side at ${where(rust)}/src/lib.rs`,
    );
    continue;
  }
  const manifest = JSON.parse(
    await readFile(join(rust, "manifest.json"), "utf8"),
  );
  const service = await serviceIdentity(plugin);
  if (service) {
    const id = manifest.id.replace(/^sikemux\./u, "");
    if (service.id !== id)
      problems.push(
        `${where(service.file)}: the service's ID is ${JSON.stringify(service.id)}, but its manifest's id ${manifest.id} says "${id}"`,
      );
    if (service.name !== manifest.name)
      problems.push(
        `${where(service.file)}: the service's NAME is ${JSON.stringify(service.name)}, but its manifest names it "${manifest.name}"`,
      );
  }
  const methods = await served(plugin);
  if (methods.size === 0) {
    problems.push(
      `${where(rust)}/src: found no \`match method {\` arms, so nothing it serves could be read`,
    );
    continue;
  }
  const files = pluginFiles.get(plugin);
  const asks = [
    ...files.flatMap(({ file, text }) =>
      asked(text, takers).map((call) => ({ ...call, file })),
    ),
    ...sharedAsks(files.map(({ text }) => text).join("\n")),
  ];
  for (const { method, file, line } of asks) {
    if (!methods.has(method))
      problems.push(
        `${where(file)}:${line}: asks ${plugin} for "${method}", which ${where(rust)}/src does not serve`,
      );
  }
  for (const tool of manifest.tools ?? []) {
    if (tool.method && !methods.has(tool.method)) {
      problems.push(
        `${where(rust)}/manifest.json: the tool ${tool.name} runs "${tool.method}", which ${where(rust)}/src does not serve`,
      );
    }
  }
}

if (problems.length > 0) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log(
  "✓ every plugin's TypeScript, tools and service name agree with its Rust side",
);
