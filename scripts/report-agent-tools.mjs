// What the agent tool surface costs. Tool schemas are sent on every request, so
// their size is a running tax; the guide's opening is fetched once and its
// topics on demand. Run this when changing a description to see the bill before
// and after.
import { readFile, readdir } from "node:fs/promises";
import { homedir } from "node:os";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const manifest = JSON.parse(
  await readFile(resolve(root, "browser/tools.json"), "utf8"),
);
const guide = await readFile(
  resolve(root, `browser/${manifest.guide.file}`),
  "utf8",
);

const topicHeading = /^## ([^:\n]+): /gm;
const topics = [...guide.matchAll(topicHeading)].map((match) => ({
  name: match[1],
  start: match.index,
}));
const opening = guide.slice(0, topics[0]?.start ?? guide.length);

// The guide tool is served locally rather than over the harness socket, but its
// schema is sent with all the others, so the bill includes it.
const served = [
  ...manifest.tools,
  {
    ...manifest.guide,
    properties: {
      topic: { enum: [...topics.map((topic) => topic.name), "all"] },
    },
    required: [],
  },
];
// The sidecar checks length limits itself and leaves them out of what it sends.
const advertised = (schema) =>
  Array.isArray(schema)
    ? schema.map(advertised)
    : schema && typeof schema === "object"
      ? Object.fromEntries(
          Object.entries(schema)
            .filter(([key]) => key !== "maxLength" && key !== "minLength")
            .map(([key, value]) => [key, advertised(value)]),
        )
      : schema;
const declared = (tools) =>
  tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    inputSchema: {
      type: "object",
      properties: advertised(tool.properties ?? {}),
      required: tool.required ?? [],
      additionalProperties: false,
    },
  }));

const perRequest = JSON.stringify(declared(served)).length;
const prose = served.reduce(
  (total, tool) => total + tool.description.length,
  0,
);
const approxTokens = (bytes) => Math.round(bytes / 4);

console.log(`tools               ${served.length}`);
console.log(
  `sent every request  ${perRequest} bytes  (~${approxTokens(perRequest)} tokens)`,
);
console.log(
  `  of which prose    ${prose} bytes  (~${approxTokens(prose)} tokens)`,
);
console.log(
  `read once, on call  ${opening.length} bytes  (~${approxTokens(opening.length)} tokens)  opening of ${manifest.guide.file}`,
);
console.log("\nguide topics, read when asked for");
topics.forEach((topic, index) => {
  const bytes = (topics[index + 1]?.start ?? guide.length) - topic.start;
  console.log(`  ${String(bytes).padStart(5)}  ${topic.name}`);
});
console.log("\nlongest descriptions");
for (const tool of [...served]
  .sort((a, b) => b.description.length - a.description.length)
  .slice(0, 5)) {
  console.log(`  ${String(tool.description.length).padStart(4)}  ${tool.name}`);
}

// A plugin's tools are sent only to agents in a repository where they would
// work, so each family is billed on its own.
const pluginsDir = resolve(root, "src-tauri/plugins");
console.log("\nplugin tools, sent where the plugin would work");
for (const entry of (await readdir(pluginsDir, { withFileTypes: true })).filter(
  (entry) => entry.isDirectory(),
)) {
  const plugin = JSON.parse(
    await readFile(resolve(pluginsDir, entry.name, "manifest.json"), "utf8"),
  );
  const tools = plugin.tools ?? [];
  if (tools.length === 0) continue;
  const bytes = JSON.stringify(declared(tools)).length;
  console.log(
    `  ${String(bytes).padStart(5)}  ${entry.name} (${tools.length} tools, ~${approxTokens(bytes)} tokens)`,
  );
}

// Sikemux counts each tool call on this machine. A tool nobody calls is a
// schema paid for on every request with nothing back.
const identifiers = ["com.nodelike.sikemux", "com.nodelike.sikemux.dev"];
const methodNames = new Map(
  manifest.tools.map((tool) => [tool.method, tool.name]),
);
for (const identifier of identifiers) {
  const path = resolve(
    homedir(),
    "Library/Application Support",
    identifier,
    "agent-tool-calls.json",
  );
  let tallies;
  try {
    tallies = JSON.parse(await readFile(path, "utf8"));
  } catch {
    continue;
  }
  const counted = new Map(
    Object.entries(tallies).map(([key, tally]) => [
      methodNames.get(key) ?? key,
      tally,
    ]),
  );
  console.log(`\ncalls counted by ${identifier}`);
  for (const [name, tally] of [...counted].sort(
    (a, b) => b[1].calls - a[1].calls,
  )) {
    const failed = tally.failures > 0 ? `  (${tally.failures} failed)` : "";
    console.log(`  ${String(tally.calls).padStart(5)}  ${name}${failed}`);
  }
  const unused = manifest.tools
    .map((tool) => tool.name)
    .filter((name) => !counted.has(name));
  if (unused.length > 0) console.log(`  never called: ${unused.join(", ")}`);
}
