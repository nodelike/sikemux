// Emits the native method lists from browser/tools.json and proves every declared
// method still has a handler. Agent tools span the Rust backend and the TypeScript
// harness; without this check a method can exist on one side and be unreachable
// from the other.
import { execFileSync } from "node:child_process";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const manifestPath = resolve(root, "browser/tools.json");
const rustOutputPath = resolve(
  root,
  "src-tauri/crates/sikemux-core/src/cli/methods.rs",
);
const browserHandlersPath = resolve(
  root,
  "src-tauri/crates/sikemux-browser/src/tools.rs",
);
const simulatorHandlersPath = resolve(
  root,
  "src-tauri/crates/sikemux-simulator/src/tools.rs",
);
const windowHandlersPath = resolve(
  root,
  "src-tauri/crates/sikemux-gpui/src/desk.rs",
);
const pluginsPath = resolve(root, "src-tauri/plugins");
const rowsOutputPath = resolve(
  root,
  "src-tauri/crates/sikemux-fold/src/tool_rows.rs",
);
const rowIconsPath = resolve(root, "shared/chat/toolRows.ts");
const wireTypesPath = resolve(root, "shared/wire.generated.ts");
const coreHandlersPath = resolve(
  root,
  "src-tauri/crates/sikemux-core/src/server/harness.rs",
);
const cliProtocolPath = resolve(
  root,
  "src-tauri/crates/sikemux-core/src/cli/protocol.rs",
);
const check = process.argv.includes("--check");

const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const methods = manifest.tools.map((tool) => tool.method);
const names = manifest.tools.map((tool) => tool.name);

if (methods.length === 0)
  throw new Error("browser/tools.json declares no tools");
if (new Set(methods).size !== methods.length)
  throw new Error("browser/tools.json declares a duplicate method");
if (new Set(names).size !== names.length)
  throw new Error("browser/tools.json declares a duplicate tool name");

const browserMethods = methods.filter((method) =>
  method.startsWith("browser."),
);
const simulatorMethods = methods.filter((method) => method.startsWith("sim."));
const harnessMethods = methods.filter(
  (method) => !method.startsWith("browser.") && !method.startsWith("sim."),
);

// A Rust arm may list several methods ("browser.back" | "browser.forward" => ...),
// so collect every literal in the pattern, not just the one next to the arrow.
function handlersIn(source, pattern, prefix) {
  const handled = new Set();
  for (const match of source.matchAll(pattern)) {
    for (const [, method] of match[1].matchAll(/"([a-zA-Z0-9.]+)"/gu)) {
      if (method.startsWith(prefix)) handled.add(method);
    }
  }
  return handled;
}

function compare(declared, handled, where) {
  const missing = declared.filter((method) => !handled.has(method));
  const orphaned = [...handled].filter((method) => !declared.includes(method));
  if (missing.length > 0)
    throw new Error(
      `${where} has no handler for ${missing.join(", ")}; implement it or drop it from browser/tools.json`,
    );
  if (orphaned.length > 0)
    throw new Error(
      `${where} handles ${orphaned.join(", ")}, which no tool exposes; declare it in browser/tools.json or delete the handler`,
    );
}

compare(
  browserMethods,
  handlersIn(
    await readFile(browserHandlersPath, "utf8"),
    /((?:"[a-z.]+"\s*\|\s*)*"[a-z.]+")\s*=>/gu,
    "browser.",
  ),
  "src-tauri/crates/sikemux-browser/src/tools.rs",
);
compare(
  simulatorMethods,
  handlersIn(
    await readFile(simulatorHandlersPath, "utf8"),
    /((?:"[a-zA-Z0-9.]+"\s*\|\s*)*"[a-zA-Z0-9.]+")\s*=>/gu,
    "sim.",
  ),
  "src-tauri/crates/sikemux-simulator/src/tools.rs",
);
// The core answers some harness methods itself and hands the rest to the
// window, so a method counts as handled by either side's dispatch.
const coreSource = await readFile(coreHandlersPath, "utf8");
const coreDispatch = coreSource.slice(
  coreSource.indexOf("async fn dispatch("),
  coreSource.indexOf("\n}\n", coreSource.indexOf("async fn dispatch(")),
);
// The core may also look at a browser or simulator call on its way to the window.
// Plugin tools are offered per plugin at run time, so `PLUGIN_METHODS` lists their methods.
const coreHandled = handlersIn(
  coreDispatch,
  /((?:"[a-z.]+"\s*\|\s*)*"[a-z.]+")\s*=>/gu,
  "",
);
const pluginMethods = handlersIn(
  await readFile(cliProtocolPath, "utf8"),
  /PLUGIN_METHODS: &\[&str\] = &\[([^\]]*)\]/gu,
  "",
);
for (const method of pluginMethods) coreHandled.delete(method);
for (const method of coreHandled) {
  if (!method.startsWith("browser.") && !method.startsWith("sim.")) continue;
  if (!methods.includes(method))
    throw new Error(
      `the core's harness dispatch handles ${method}, which no tool exposes`,
    );
  coreHandled.delete(method);
}
const windowSource = await readFile(windowHandlersPath, "utf8");
const windowHandled = handlersIn(
  windowSource,
  /^\s*("[a-z]+\.[a-z]+")\s*=>/gmu,
  "",
);
compare(
  harnessMethods,
  new Set([...coreHandled, ...windowHandled]),
  "the core's harness dispatch and the GPUI window's in sikemux-gpui/src/desk.rs",
);

// A method the core hands on to the window needs a GPUI handler, unless the core
// answers by itself when the window cannot.
const functionBody = (name) => {
  const start = coreSource.indexOf(`async fn ${name}(`);
  return start < 0
    ? ""
    : coreSource.slice(start, coreSource.indexOf("\n}\n", start));
};
const armBody = (method) => {
  const arm = new RegExp(
    `\\n {8}(?:"[a-z.]+" \\| )*"${method.replaceAll(".", "\\.")}"[^\\n]*=>`,
    "u",
  ).exec(coreDispatch);
  if (!arm) return null;
  const rest = coreDispatch.slice(arm.index + arm[0].length);
  const next = /\n {8}(?:"[a-z.]+"|[A-Z_]+|_)\s*(?:\||=>)/u.exec(rest);
  const body = next ? rest.slice(0, next.index) : rest;
  const called = [...body.matchAll(/\b([a-z_]+)\(core,/gu)].map(([, name]) =>
    functionBody(name),
  );
  return [body, ...called].join("\n");
};
const missingInWindow = harnessMethods.filter((method) => {
  const body = armBody(method);
  const forwarded = body === null || body.includes("forward(");
  const fallsBack =
    body !== null && /forward\(core, request\)\.await\.ok\(\)/u.test(body);
  return forwarded && !fallsBack && !windowHandled.has(method);
});
if (missingInWindow.length > 0)
  throw new Error(
    `the core hands ${missingInWindow.join(", ")} to the window, which has no GPUI handler in sikemux-gpui/src/desk.rs`,
  );
for (const [prefix, dispatcher] of [
  ["browser.", "is_browser_method"],
  ["sim.", "is_sim_method"],
])
  if (!windowSource.includes(`${dispatcher}(&request.method)`))
    throw new Error(
      `the GPUI window no longer hands ${prefix}* calls on through ${dispatcher}`,
    );

const list = (values) =>
  values.map((value) => `    ${JSON.stringify(value)},`).join("\n");
const rustOutput = `// Generated by scripts/generate-agent-tools.mjs from browser/tools.json. Do not edit by hand.
pub const BROWSER_METHODS: &[&str] = &[
${list(browserMethods)}
];

pub const SIM_METHODS: &[&str] = &[
${list(simulatorMethods)}
];

pub const HARNESS_METHODS: &[&str] = &[
${list(harnessMethods)}
];
`;

async function emit(path, contents) {
  let actual = null;
  try {
    actual = await readFile(path, "utf8");
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  if (actual === contents) return;
  if (check) throw new Error(`Generated agent tool contract is stale: ${path}`);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, contents);
}

await emit(rustOutputPath, rustOutput);

// Every tool says how the chat pane draws a call to it, so a new tool or plugin
// never shows up there as a bare server and function name.
const plugins = [];
for (const entry of await readdir(pluginsPath, { withFileTypes: true })) {
  if (!entry.isDirectory()) continue;
  const path = resolve(pluginsPath, entry.name, "manifest.json");
  const plugin = JSON.parse(await readFile(path, "utf8"));
  plugins.push({
    where: `src-tauri/plugins/${entry.name}/manifest.json`,
    plugin: plugin.id,
    tools: plugin.tools ?? [],
  });
}
const unionOf = (source, type) =>
  new Set(
    [
      ...new RegExp(`export type ${type} =([^;]+);`, "u")
        .exec(source)[1]
        .matchAll(/"([a-z_-]+)"/gu),
    ].map(([, value]) => value),
  );
const icons = unionOf(await readFile(rowIconsPath, "utf8"), "ToolRowIcon");
const kinds = unionOf(await readFile(wireTypesPath, "utf8"), "ToolAction");
const described = [
  {
    where: "browser/tools.json",
    tools: [
      {
        ...manifest.guide,
        properties: { topic: {} },
      },
      ...manifest.tools,
    ],
  },
  ...plugins,
];
const rows = {};
for (const { where, plugin, tools } of described) {
  for (const tool of tools) {
    const fail = (problem) => {
      throw new Error(`${where}: ${tool.name} ${problem}`);
    };
    const { row } = tool;
    if (!row) fail('has no "row" saying how the chat pane draws it');
    const extra = Object.keys(row).filter(
      (key) => !["verb", "kind", "icon", "target", "detail"].includes(key),
    );
    if (extra.length > 0) fail(`row has unknown keys: ${extra.join(", ")}`);
    if (!/^[a-z]{2,8}$/u.test(row.verb ?? ""))
      fail("row verb must be one lowercase word of 2 to 8 letters");
    if (!kinds.has(row.kind))
      fail(
        `row kind "${row.kind}" is not one of ${[...kinds].join(", ")} (ToolAction in sikemux-wire)`,
      );
    if (!icons.has(row.icon))
      fail(
        `row icon "${row.icon}" is not one of ${[...icons].join(", ")} (shared/chat/toolRows.ts)`,
      );
    for (const key of ["target", "detail"]) {
      const templates = row[key];
      if (templates === undefined && key === "detail") continue;
      if (
        !Array.isArray(templates) ||
        templates.length === 0 ||
        !templates.every((template) => typeof template === "string")
      )
        fail(`row ${key} must be a list of templates`);
      for (const template of templates) {
        for (const [, argument] of template.matchAll(/\{(\w+)\}/gu)) {
          if (!Object.hasOwn(tool.properties ?? {}, argument))
            fail(
              `row ${key} "${template}" names {${argument}}, which the tool does not take`,
            );
        }
      }
    }
    if (Object.hasOwn(rows, tool.name)) fail("is declared twice");
    rows[tool.name] = plugin ? { ...row, plugin } : row;
  }
}
const templates = (list) =>
  `&[${(list ?? []).map((template) => JSON.stringify(template)).join(", ")}]`;
const specs = Object.entries(rows)
  .map(
    ([name, row]) =>
      `(${JSON.stringify(name)}, Spec { verb: ${JSON.stringify(row.verb)}, kind: ${JSON.stringify(row.kind)}, icon: ${JSON.stringify(row.icon)}, target: ${templates(row.target)}, detail: ${templates(row.detail)}, plugin: ${row.plugin ? `Some(${JSON.stringify(row.plugin)})` : "None"} }),`,
  )
  .join("\n");
const rowsSource = `// Generated by scripts/generate-agent-tools.mjs from browser/tools.json and the plugin manifests. Do not edit by hand.
use super::row::Spec;

pub(super) const SPECS: &[(&str, Spec)] = &[
${specs}
];
`;
await emit(
  rowsOutputPath,
  execFileSync("rustfmt", ["--edition", "2021"], {
    input: rowsSource,
    encoding: "utf8",
  }),
);

console.log(
  `✓ ${methods.length} agent tools: ${browserMethods.length} browser, ${simulatorMethods.length} simulator, ${harnessMethods.length} harness; ${Object.keys(rows).length} chat rows`,
);
