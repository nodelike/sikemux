#!/usr/bin/env node
// Writes the types clients read the core's JSON with, from the Rust that
// serializes it: TypeScript for the phone and the web, and Swift for the notch.
// It reads the serde declarations itself and fails on any it does not know,
// rather than guess. `--check` fails when an output is out of date.

import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { format, resolveConfig } from "prettier";

const root = resolve(import.meta.dirname, "..");
const check = process.argv.includes("--check");

const WIRE = "crates/sikemux-wire/src";
const VIEW = "crates/sikemux-view/src";
const CORE = "crates/sikemux-core/src";
const PLUGIN_API = "crates/sikemux-plugin-api/src";
const MARKDOWN = "crates/sikemux-markdown/src";
const BASE = "crates/sikemux-base/src";
const SOURCES = (
  await Promise.all(
    [WIRE, VIEW, CORE, PLUGIN_API, MARKDOWN, BASE].map((crate) =>
      readdir(resolve(root, crate), { recursive: true }),
    ),
  )
).flatMap((files, index) =>
  files
    .filter((file) => file.endsWith(".rs"))
    .map(
      (file) =>
        `${[WIRE, VIEW, CORE, PLUGIN_API, MARKDOWN, BASE][index]}/${file}`,
    ),
);

/** Fields the Rust keeps as JSON so phones read changes over the air, and the type that JSON has. */
const JSON_FIELDS = {
  "ChatAttachment.view": "ChatView",
  "ChatAttachment.changes": "ChatChange[]",
  "PartTree.blocks": "MdElement[]",
  "ChatChange.md": "MdChange",
  "Event.view": "DeviceView",
  "Event.topic": "Topic",
};

const TS_ROOTS = [
  "NotificationCard",
  "DeviceView",
  "ChatView",
  "ChatPage",
  "ChatChange",
  "ToolDetail",
  "Preferences",
  "Palette",
  "ProjectBoard",
  "HostInfo",
  "RemoteStatus",
  "RemotePairing",
  "NetworkChange",
  "Inbox",
  "AgentQuestion",
  "CatalogAgent",
  "Draft",
  "UpdateStatus",
  "ShareFrame",
  "ShareAsk",
  "ShareReply",
  "MdChange",
  "MarkdownOptions",
];

/**
 * What the notch reads of the core, which it says itself: the fields of each type (Swift's
 * decoder ignores the rest), the requests by their `ask` struct, the calls by method, the events,
 * and the types it declares in helpers/notch/settings.json.
 */
const {
  fields: SWIFT_FIELDS,
  roots: SWIFT_ROOTS,
  requests: SWIFT_REQUESTS,
  calls: SWIFT_CALLS,
  events: SWIFT_EVENTS,
  declared: SWIFT_DECLARED,
} = JSON.parse(
  await readFile(resolve(root, "helpers/notch/wire.json"), "utf8"),
);

/** The calls the phone makes, which it says itself: the Mac reads the Rust, so nothing else reads the table. */
const { calls: PHONE_CALLS } = JSON.parse(
  await readFile(resolve(root, "mobile/app/wire.json"), "utf8"),
);

/** Limits the clients check before they send, read from where the core enforces them. */
const LIMITS = [
  [`${WIRE}/protocol.rs`, "MAX_ATTACHMENT_BYTES"],
  [`${CORE}/server/chat/prompt.rs`, "MAX_ATTACHMENTS"],
];

/** How long the host waits on a client, in seconds, read from where the host waits. */
const WAITS = [["crates/sikemux-client/src/join.rs", "APPROVAL_TIMEOUT"]];

/** Addresses clients write into what they send, read from the core's own. */
const URIS = ["AGENT_URI", "EARLIER_URI", "STASH_URI", "CHAT_DRAFT"];

const OUTPUTS = {
  ts: "shared/wire.generated.ts",
  phone: "mobile/ios/Sikemux/Core/Wire.generated.swift",
  notch: "helpers/notch/Sources/NotchKit/Wire.generated.swift",
  cardSwift: "mobile/ios/Shared/Notify/Card.generated.swift",
  glanceSwift: "mobile/ios/Shared/Glance/Glance.generated.swift",
  cardKotlin:
    "mobile/app/modules/notify/android/src/main/java/com/nodelike/sikemux/notify/Card.generated.kt",
};

/** What only the phone's Swift reads or sends: settings inside JSON the bridge passes through, and the kinds of refusal. */
const PHONE_ROOTS = ["NotifyPrefs", "ErrorKind"];

/** What the phone's own notification code reads of a sealed card, in Swift and Kotlin. */
const CARD_ROOTS = ["NotificationCard"];

/** What the phone's widgets and Live Activity read of a host's agents, in Swift as the app declares it. */
const GLANCE_ROOTS = ["ActivityGlance"];
/** The glance's declarations, written by phoneSwift for the extensions that do not compile the whole view. */
let glanceSwift = "";

const fail = (message) => {
  throw new Error(`generate-wire-types: ${message}`);
};

function balanced(text, start) {
  const pairs = { "(": ")", "[": "]", "{": "}", "<": ">" };
  const stack = [];
  for (let index = start; index < text.length; index += 1) {
    const char = text[index];
    if (char === '"' || text.startsWith("//", index)) {
      index = text.indexOf(char === '"' ? '"' : "\n", index + 1);
      continue;
    }
    if (pairs[char]) stack.push(pairs[char]);
    else if (char === stack.at(-1)) {
      stack.pop();
      if (stack.length === 0) return index + 1;
    } else if (")]}".includes(char) && char !== ">") {
      fail(`unbalanced ${char} in ${text.slice(start, index + 1)}`);
    }
  }
  fail(`unclosed bracket in ${text.slice(start, start + 80)}`);
}

function splitTop(text) {
  const parts = [];
  let depth = 0;
  let from = 0;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === '"') index = text.indexOf('"', index + 1);
    else if (text.startsWith("//", index)) index = text.indexOf("\n", index);
    else if ("([{<".includes(char)) depth += 1;
    else if (")]}".includes(char) || (char === ">" && text[index - 1] !== "-"))
      depth -= 1;
    else if (char === "," && depth === 0) {
      parts.push(text.slice(from, index));
      from = index + 1;
    }
  }
  parts.push(text.slice(from));
  return parts.map((part) => part.trim()).filter(Boolean);
}

/** Doc lines, attributes and the rest, for one member or item. */
function readMember(text) {
  const docs = [];
  const attrs = [];
  let rest = text.trim();
  for (;;) {
    if (rest.startsWith("///")) {
      const end = rest.indexOf("\n");
      docs.push(rest.slice(3, end < 0 ? undefined : end).replace(/^ /, ""));
      rest = end < 0 ? "" : rest.slice(end + 1).trim();
    } else if (rest.startsWith("//")) {
      const end = rest.indexOf("\n");
      rest = end < 0 ? "" : rest.slice(end + 1).trim();
    } else if (rest.startsWith("#[")) {
      const end = balanced(rest, 1);
      attrs.push(rest.slice(2, end - 1).trim());
      rest = rest.slice(end).trim();
    } else return { docs, attrs, rest };
  }
}

/** The serde settings among `attrs`, as `{ key: value | true }`. */
function serde(attrs, where) {
  const settings = {};
  for (const attr of attrs) {
    if (attr.startsWith("cfg")) fail(`${where} is compiled conditionally`);
    if (!attr.startsWith("serde(")) continue;
    for (const part of splitTop(attr.slice(6, -1))) {
      const match = /^(\w+)(?:\s*=\s*"([^"]*)")?$/.exec(part);
      if (!match) fail(`${where} has serde setting ${part}`);
      settings[match[1]] = match[2] ?? true;
    }
  }
  return settings;
}

const KNOWN = {
  container: [
    "rename_all",
    "default",
    "tag",
    "rename_all_fields",
    "untagged",
    "deny_unknown_fields",
  ],
  field: [
    "rename",
    "default",
    "skip_serializing_if",
    "skip",
    "alias",
    "deserialize_with",
    "flatten",
  ],
  variant: ["rename", "other", "alias", "untagged"],
};

function only(settings, allowed, where) {
  for (const key of Object.keys(settings))
    if (!allowed.includes(key)) fail(`${where} uses serde(${key})`);
}

/** Every top-level struct, enum and type alias in a file, unparsed. */
function items(file, text) {
  const found = new Map();
  const declaration =
    /^(pub(?:\([^)]*\))?\s+)?(struct|enum|type)\s+(\w+)(<[^>]*>)?/;
  const lines = text.split("\n");
  let pending = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (/^(\/\/\/|#\[)/.test(line)) {
      pending.push(line);
      if (line.startsWith("#[")) {
        let attr = line;
        while (
          [...attr].filter((c) => c === "[").length >
          [...attr].filter((c) => c === "]").length
        ) {
          index += 1;
          attr += `\n${lines[index]}`;
          pending.push(lines[index]);
        }
      }
      continue;
    }
    const match = declaration.exec(line);
    if (match) {
      const rest = lines.slice(index).join("\n");
      const head =
        match[0].length + rest.slice(match[0].length).search(/[{;(=]/);
      const end =
        rest[head] === "{" || rest[head] === "("
          ? balanced(rest, head)
          : rest.indexOf(";") + 1;
      const { docs, attrs } = readMember(pending.join("\n"));
      found.set(match[3], {
        file,
        name: match[3],
        kind: match[2],
        public: match[1] !== undefined,
        generics: match[4],
        docs,
        attrs,
        body: rest.slice(head, end),
      });
    }
    if (line.trim() !== "") pending = [];
  }
  return found;
}

const texts = await Promise.all(
  SOURCES.map((file) => readFile(resolve(root, file), "utf8")),
);
const sources = SOURCES.map((file, index) => items(file, texts[index]));

const limitValues = await Promise.all(
  LIMITS.map(async ([file, name]) => {
    const text = await readFile(resolve(root, file), "utf8");
    const value = new RegExp(
      `^(?:pub )?const ${name}: (?:usize|u32|f64) = ([\\d_. *]+);$`,
      "m",
    ).exec(text)?.[1];
    if (!value) fail(`${file} has no number constant ${name}`);
    const product = value
      .split("*")
      .reduce(
        (total, part) => total * Number(part.trim().replaceAll("_", "")),
        1,
      );
    return [name, product];
  }),
);
const limits = limitValues
  .map(([name, product]) => `export const ${name} = ${product};`)
  .join("\n");

const waitValues = await Promise.all(
  WAITS.map(async ([file, name]) => {
    const text = await readFile(resolve(root, file), "utf8");
    const value = new RegExp(
      `^(?:pub )?const ${name}: Duration = Duration::from_secs\\((\\d+)\\);$`,
      "m",
    ).exec(text)?.[1];
    if (!value) fail(`${file} has no Duration::from_secs constant ${name}`);
    return [`${name}_SECONDS`, Number(value)];
  }),
);
const waits = waitValues
  .map(([name, seconds]) => `export const ${name} = ${seconds};`)
  .join("\n");

const importsOf = new Map();

/** The path each name a file imports comes from, as its `use` lines write it. */
function imported(file) {
  if (importsOf.has(file)) return importsOf.get(file);
  const found = new Map();
  const text = texts[SOURCES.indexOf(file)];
  for (const [, path, group] of text.matchAll(
    /^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+([\w:]+)::(\w+|\{[^}]*\})\s*;/gm,
  ))
    for (const name of group.replace(/[{}]/g, "").split(","))
      found.set(name.trim().split(/\s+/)[0], path.split("::"));
  importsOf.set(file, found);
  return found;
}

/**
 * The item `name` names in `file`: its own, else the one source that declares it. `modules`,
 * the path the Rust wrote before the name, such as `crate::fuzzy`, picks among several.
 */
function lookup(file, name, modules = []) {
  const own = sources[SOURCES.indexOf(file)].get(name);
  if (own && !modules.length) return own;
  if (!modules.length) modules = imported(file).get(name) ?? [];
  let all = sources.flatMap((source) =>
    source.has(name) && source.get(name).public ? [source.get(name)] : [],
  );
  const path = modules
    .filter(
      (part) =>
        !["crate", "super", "self"].includes(part) &&
        !part.startsWith("sikemux_"),
    )
    .join("/");
  const fromPath = all.filter(
    (item) =>
      item.file.endsWith(`/${path}.rs`) ||
      item.file.endsWith(`/${path}/mod.rs`) ||
      item.file.includes(`/${path}/`),
  );
  if (all.length > 1 && path && fromPath.length) all = fromPath;
  const crate = file.split("/src/")[0];
  const folder = dirname(file);
  const nearest = [
    (item) =>
      dirname(item.file) === folder ||
      [`${folder}.rs`, `${folder}/mod.rs`].includes(item.file),
    (item) => item.file.startsWith(`${crate}/`),
    (item) => item.file.startsWith(WIRE) || item.file.startsWith(VIEW),
  ].map((near) => all.filter(near));
  all = nearest.find((near) => near.length) ?? all;
  if (all.length > 1)
    fail(`${name} in ${file} could be any of ${all.map((i) => i.file)}`);
  return all[0] ?? own;
}

const declared = new Map();
for (const source of sources)
  for (const item of source.values()) {
    if (!item.public) continue;
    const name = item.name;
    declared.set(name, [...(declared.get(name) ?? []), item]);
  }

function byClientName(name) {
  const all = declared.get(name) ?? [];
  // The contract's own crates hold the type a client means by a name.
  const contract = all.filter(
    (item) => item.file.startsWith(WIRE) || item.file.startsWith(VIEW),
  );
  const found = contract.length ? contract : all;
  if (found.length !== 1)
    fail(
      `${name} is declared ${found.length} times: ${found.map((item) => item.file)}`,
    );
  return found[0];
}

/** Each of URIS with its value, as the Rust writes the string. */
async function uriValues() {
  const text = await readFile(resolve(root, `${VIEW}/chat.rs`), "utf8");
  return URIS.map((name) => {
    const value = new RegExp(
      `^pub const ${name}: &str = ("[^"]+");$`,
      "m",
    ).exec(text)?.[1];
    if (!value) fail(`${VIEW}/chat.rs has no text constant ${name}`);
    return [name, value];
  });
}

async function uris() {
  return (await uriValues())
    .map(([name, value]) => `export const ${name} = ${value};`)
    .join("\n");
}

/** The bare names a project spells out in full, which are files without an extension, as the Rust lists them. */
async function extensionlessNames() {
  const text = await readFile(resolve(root, `${MARKDOWN}/paths.rs`), "utf8");
  const names = /pub const EXTENSIONLESS: \[&str; \d+\] = \[([^\]]*)\]/.exec(
    text,
  )?.[1];
  if (!names) fail(`${MARKDOWN}/paths.rs has no EXTENSIONLESS`);
  return names;
}

async function extensionless() {
  return `export const EXTENSIONLESS: readonly string[] = [${await extensionlessNames()}];`;
}

const PRIMITIVES = {
  String: "string",
  PathBuf: "string",
  str: "string",
  bool: "bool",
  f32: "float",
  f64: "float",
  Value: "json",
  ...Object.fromEntries(
    "u8 u16 u32 u64 usize i8 i16 i32 i64 isize"
      .split(" ")
      .map((name) => [name, "int"]),
  ),
};

/** A Rust type as `{ kind, ... }`: primitive, option, list, map, tuple or named. */
function parseType(text, file) {
  text = text.trim().replace(/^&(?:'\w+\s+)?/, "");
  if (text === "()") return { kind: "null" };
  const array = /^\[(.+);\s*(\d+)\]$/s.exec(text);
  if (array)
    return {
      kind: "tuple",
      of: Array.from({ length: Number(array[2]) }, () =>
        parseType(array[1], file),
      ),
    };
  if (/^\(.*\)$/s.test(text))
    return {
      kind: "tuple",
      of: splitTop(text.slice(1, -1)).map((part) => parseType(part, file)),
    };
  const generic = /^([\w:]+)\s*<(.*)>$/s.exec(text);
  const segments = (generic?.[1] ?? text).split("::");
  const path = segments.at(-1);
  const args = generic ? splitTop(generic[2]) : [];
  const one = () => {
    if (args.length !== 1) fail(`${text} needs one type argument`);
    return parseType(args[0], file);
  };
  if (path === "Option") return { kind: "option", of: one() };
  if (path === "Vec") return { kind: "list", of: one() };
  if (path === "Box") return one();
  if (path === "Cow") {
    const owned = args.filter((arg) => !arg.startsWith("'"));
    if (owned.length !== 1) fail(`${text} needs one type argument`);
    return parseType(owned[0] === "str" ? "String" : owned[0], file);
  }
  if (path === "BTreeMap" || path === "HashMap" || path === "Map") {
    if (args.length !== 2 || parseType(args[0], file).kind !== "string")
      fail(`${text} is not keyed by strings`);
    return { kind: "map", of: parseType(args[1], file) };
  }
  if (generic) fail(`${text} is generic`);
  if (PRIMITIVES[path]) return { kind: PRIMITIVES[path] };
  const item = lookup(file, path, segments.slice(0, -1));
  if (!item) fail(`${text} in ${file} is not declared in any source`);
  if (item.kind === "type") {
    const alias = /^=\s*(.*);$/s.exec(item.body);
    return parseType(alias[1], item.file);
  }
  return { kind: "named", name: item.name };
}

const words = (name) =>
  name
    .replace(/^r#/, "")
    .split("_")
    .flatMap((part) => part.split(/(?=[A-Z])/))
    .filter(Boolean)
    .map((word) => word.toLowerCase());

function renameAll(name, rule, where) {
  const parts = words(name);
  const cap = (word) => word[0].toUpperCase() + word.slice(1);
  switch (rule) {
    case undefined:
      return name.replace(/^r#/, "");
    case "camelCase":
      return parts[0] + parts.slice(1).map(cap).join("");
    case "snake_case":
      return parts.join("_");
    case "kebab-case":
      return parts.join("-");
    case "lowercase":
      return parts.join("");
    default:
      fail(`${where} renames all to ${rule}`);
  }
}

/** The fields a `serde(flatten)` field puts beside its neighbours in the JSON. */
function flattened(type, where) {
  if (type.kind !== "named") fail(`${where} flattens what is not a struct`);
  const inner = parse(byClientName(type.name));
  if (inner.kind !== "struct") fail(`${where} flattens what is not a struct`);
  return inner.fields;
}

function parseFields(body, file, rule, where) {
  return splitTop(body.slice(1, -1)).flatMap((member) => {
    const { docs, attrs, rest } = readMember(member);
    if (!rest) return [];
    const match = /^(?:pub(?:\([^)]*\))?\s+)?([\w#]+)\s*:\s*(.+)$/s.exec(rest);
    if (!match) fail(`${where} has field ${rest}`);
    const settings = serde(attrs, `${where}.${match[1]}`);
    only(settings, KNOWN.field, `${where}.${match[1]}`);
    if (settings.skip) return [];
    const type = parseType(match[2], file);
    if (settings.flatten && type.kind === "map") return [];
    if (settings.flatten) return flattened(type, `${where}.${match[1]}`);
    return {
      name: settings.rename ?? renameAll(match[1], rule, where),
      rust: match[1],
      docs,
      type,
      mayBeAbsent: settings.skip_serializing_if !== undefined,
      defaulted: settings.default !== undefined,
    };
  });
}

/** Gives the fields the Rust keeps as JSON the type that JSON has, as JSON_FIELDS names it. */
function typeJson(name, fields) {
  for (const field of fields) {
    const json = JSON_FIELDS[`${name}.${field.name}`];
    if (!json) continue;
    const optional = field.type.kind === "option";
    const inner = optional ? field.type.of : field.type;
    if (inner.kind !== "json") fail(`${name}.${field.name} is not JSON`);
    const named = json.endsWith("[]")
      ? { kind: "list", of: { kind: "named", name: json.slice(0, -2) } }
      : { kind: "named", name: json };
    field.type = optional ? { kind: "option", of: named } : named;
  }
}

/** A declaration as the clients see its JSON; of a tagged enum, only the variants in `wanted` when given. */
function parse(item, wanted) {
  const where = `${item.name} in ${item.file}`;
  if (item.generics) fail(`${where} is generic`);
  const settings = serde(item.attrs, where);
  only(settings, KNOWN.container, where);
  const name = item.name;
  if (item.kind === "struct") {
    if (!item.body.startsWith("{"))
      fail(`${where} is not a struct with named fields`);
    const fields = parseFields(
      item.body,
      item.file,
      settings.rename_all,
      where,
    );
    typeJson(name, fields);
    return { name, docs: item.docs, kind: "struct", fields };
  }
  if (item.kind !== "enum") fail(`${where} is a ${item.kind}`);
  if (settings.tag) {
    const members = splitTop(item.body.slice(1, -1)).filter(
      (member) =>
        !wanted || wanted.includes(/^\w+/.exec(readMember(member).rest)?.[0]),
    );
    const variants = members.map((member) => {
      const { docs, attrs, rest } = readMember(member);
      const match = /^(\w+)\s*(\{[\s\S]*\})?$/.exec(rest);
      if (!match) fail(`${where}::${rest} is not a unit or struct variant`);
      const variant = serde(attrs, `${where}::${match[1]}`);
      only(variant, KNOWN.variant, `${where}::${match[1]}`);
      const fields = match[2]
        ? parseFields(
            match[2],
            item.file,
            settings.rename_all_fields,
            `${where}::${match[1]}`,
          )
        : [];
      typeJson(name, fields);
      return {
        name: variant.rename ?? renameAll(match[1], settings.rename_all, where),
        docs,
        fields,
      };
    });
    return {
      name,
      docs: item.docs,
      kind: "tagged",
      tag: settings.tag,
      variants,
    };
  }
  if (settings.rename_all_fields)
    fail(`${where} renames fields but has no tag`);
  if (settings.untagged)
    return {
      name,
      docs: item.docs,
      kind: "untagged",
      variants: splitTop(item.body.slice(1, -1)).map((member) => {
        const { docs, rest } = readMember(member);
        const variantName = /^\w+/.exec(rest)?.[0];
        const held = /^\w+\s*\((.*)\)$/s.exec(rest);
        if (held)
          return {
            name: variantName,
            docs,
            type: parseType(held[1], item.file),
          };
        const fields = /^\w+\s*(\{[\s\S]*\})$/.exec(rest);
        if (!fields) fail(`${where}::${rest} holds nothing to tell it apart`);
        return {
          name: variantName,
          docs,
          fields: parseFields(fields[1], item.file, undefined, where),
        };
      }),
    };
  const members = splitTop(item.body.slice(1, -1));
  // Every variant holding one value is serde's default tagging: an object keyed by the variant.
  if (
    members.every((member) => /^\w+\s*\(.*\)$/s.test(readMember(member).rest))
  )
    return {
      name,
      docs: item.docs,
      kind: "wrapped",
      variants: members.map((member) => {
        const { docs, attrs, rest } = readMember(member);
        const [, variantName, held] = /^(\w+)\s*\((.*)\)$/s.exec(rest);
        const variant = serde(attrs, `${where}::${variantName}`);
        only(variant, KNOWN.variant, `${where}::${variantName}`);
        return {
          name:
            variant.rename ??
            renameAll(variantName, settings.rename_all, where),
          docs,
          type: parseType(held, item.file),
        };
      }),
    };
  const variants = members.map((member) => {
    const { docs, attrs, rest } = readMember(member);
    const variant = serde(attrs, `${where}::${rest}`);
    only(variant, KNOWN.variant, `${where}::${rest}`);
    // An untagged variant holding a string is any other string.
    if (variant.untagged && /^\w+\s*\(\s*String\s*\)$/.test(rest))
      return { name: rest, rust: rest, docs, other: true, open: true };
    if (!/^\w+$/.test(rest)) fail(`${where}::${rest} carries data`);
    return {
      name: variant.rename ?? renameAll(rest, settings.rename_all, where),
      rust: rest,
      docs,
      other: variant.other === true,
    };
  });
  return { name, docs: item.docs, kind: "strings", variants };
}

function named(type) {
  if (type.kind === "named") return [type.name];
  if (type.kind === "tuple") return type.of.flatMap(named);
  if (type.of) return named(type.of);
  return [];
}

/** The declarations `roots` need, each before the first that uses it, but those in `declared`. */
function closure(roots, fieldsOf = () => undefined, declared = []) {
  const order = [];
  const seen = new Set(declared);
  const visit = (name) => {
    if (seen.has(name)) return;
    seen.add(name);
    const declaration = parse(byClientName(name));
    const wanted = fieldsOf(name);
    if (declaration.kind === "struct" && wanted) {
      const missing = wanted.filter(
        (field) => !declaration.fields.some((f) => f.name === field),
      );
      if (missing.length) fail(`${name} has no ${missing.join(", ")}`);
      declaration.fields = declaration.fields.filter((field) =>
        wanted.includes(field.name),
      );
    }
    order.push(declaration);
    [
      ...(declaration.fields ?? []),
      ...(declaration.kind === "tagged"
        ? declaration.variants.flatMap((variant) => variant.fields)
        : []),
      ...(declaration.kind === "wrapped" ? declaration.variants : []),
      ...(declaration.kind === "untagged"
        ? declaration.variants.flatMap((variant) =>
            variant.fields ? variant.fields : [variant],
          )
        : []),
    ]
      .flatMap((field) => named(field.type))
      .forEach(visit);
  };
  roots.forEach(visit);
  return order;
}

function tsDocs(lines, indent = "") {
  if (lines.length === 0) return "";
  if (lines.length === 1) return `${indent}/** ${lines[0]} */\n`;
  return `${indent}/**\n${lines.map((line) => `${indent} * ${line}`.trimEnd()).join("\n")}\n${indent} */\n`;
}

/**
 * TypeScript for every client, which reads the core's JSON as it is: a `None` the
 * core sends is `null`, and a field it may leave out is optional. A field serde
 * fills in by default is optional too, since an older host leaves it out.
 */
function typescript(roots, preamble) {
  const tsType = (type) => {
    switch (type.kind) {
      case "string":
        return "string";
      case "bool":
        return "boolean";
      case "int":
      case "float":
        return "number";
      case "json":
        return "unknown";
      case "null":
        return "null";
      case "named":
        return type.name;
      case "list": {
        const of = tsType(type.of);
        return /[ |]/.test(of) ? `(${of})[]` : `${of}[]`;
      }
      case "map":
        return `Record<string, ${tsType(type.of)}>`;
      case "tuple":
        return `[${type.of.map(tsType).join(", ")}]`;
      case "option":
        return `${tsType(type.of)} | null`;
    }
  };
  const field = (f) => {
    const optional = f.mayBeAbsent || f.defaulted;
    const type = f.mayBeAbsent && f.type.kind === "option" ? f.type.of : f.type;
    return `${tsDocs(f.docs, "  ")}  ${f.name}${optional ? "?" : ""}: ${tsType(type)};`;
  };
  const tagged = (d) =>
    `${tsDocs(d.docs)}export type ${d.name} =\n${d.variants
      .map(
        (v) =>
          `${tsDocs(v.docs, "  ")}  | {\n  ${d.tag}: ${JSON.stringify(v.name)};\n${v.fields.map(field).join("\n")}\n}`,
      )
      .join("\n")};`;
  const wrapped = (d) =>
    `${tsDocs(d.docs)}export type ${d.name} =\n${d.variants
      .map((v) => `${tsDocs(v.docs, "  ")}  | { ${v.name}: ${tsType(v.type)} }`)
      .join("\n")};`;
  const untagged = (d) =>
    `${tsDocs(d.docs)}export type ${d.name} =\n${d.variants
      .map((v) =>
        v.fields
          ? `${tsDocs(v.docs, "  ")}  | {\n${v.fields.map(field).join("\n")}\n}`
          : `${tsDocs(v.docs, "  ")}  | ${tsType(v.type)}`,
      )
      .join("\n")};`;
  const declare = (d) =>
    d.kind === "untagged"
      ? untagged(d)
      : d.kind === "struct" && d.fields.length === 0
        ? `${tsDocs(d.docs)}export type ${d.name} = Record<string, never>;`
        : d.kind === "struct"
          ? `${tsDocs(d.docs)}export interface ${d.name} {\n${d.fields.map(field).join("\n")}\n}`
          : d.kind === "tagged"
            ? tagged(d)
            : d.kind === "wrapped"
              ? wrapped(d)
              : `${tsDocs(d.docs)}export type ${d.name} = ${[
                  ...d.variants
                    .filter((v) => !v.other)
                    .map((v) => JSON.stringify(v.name)),
                  ...(d.variants.some((v) => v.open) ? ["(string & {})"] : []),
                ].join(" | ")};`;
  const calls = coreCalls();
  const callMap = `/** The core's calls that name what they take and answer, by method. */\nexport interface CoreCalls {\n${calls
    .map(
      (call) =>
        `  ${JSON.stringify(call.method)}: { params: ${tsType(call.takes)}; answer: ${tsType(call.gives)} };`,
    )
    .join("\n")}\n}`;
  const used = calls.flatMap((call) => [
    ...named(call.takes),
    ...named(call.gives),
  ]);
  return `// Generated by scripts/generate-wire-types.mjs from the core's Rust. Do not edit by hand.\n\n${preamble}${closure(
    [...roots, ...used],
  )
    .map(declare)
    .join("\n\n")}\n\n${callMap}\n`;
}

/**
 * The methods the phone calls, as mobile/app/wire.json names them, from the core's `calls!` rows that name
 * their params and answer, as `"method" Marker (Params) -> Answer`.
 */
function coreCalls() {
  const file = `${CORE}/server/calls.rs`;
  const text = texts[SOURCES.indexOf(file)];
  const rows = new Map(
    [
      ...text.matchAll(
        /^\s*"([\w.]+)"\s*\w+\s*\((.*?)\)\s*->\s*(.+?):\s*[A-Z]\w*(?:,\s*\w+(?:\([^)]*\))?)*\s*=>/gm,
      ),
    ].map(([, method, takes, gives]) => [method, { takes, gives }]),
  );
  return PHONE_CALLS.map((method) => {
    const row = rows.get(method);
    if (!row)
      fail(
        `mobile/app/wire.json names ${method}, which calls.rs does not type`,
      );
    return {
      method,
      takes: parseType(row.takes, file),
      gives: parseType(row.gives, file),
    };
  });
}

const swiftCase = (name) => renameAll(name.replaceAll("-", "_"), "camelCase");

function swiftType(type) {
  switch (type.kind) {
    case "string":
      return "String";
    case "bool":
      return "Bool";
    case "int":
      return "Int";
    case "float":
      return "Double";
    case "named":
      return type.name;
    case "list":
      return `[${swiftType(type.of)}]`;
    case "map":
      return `[String: ${swiftType(type.of)}]`;
    case "option":
      return `${swiftType(type.of)}?`;
    case "null":
      return "Nothing?";
    default:
      fail(`Swift cannot read ${type.kind}`);
  }
}
const swiftDocs = (lines, indent) =>
  lines.map((line) => `${indent}/// ${line}`.trimEnd() + "\n").join("");
/** A Swift declaration; the types in `sent` are sent, so they may leave out what is optional. */
function swiftDeclare(d, sent = new Set()) {
  if (d.kind === "struct") {
    const fields = d.fields.map((f) => {
      if (!/^[a-z]\w*$/.test(f.name))
        fail(`${d.name}.${f.name} is no Swift name`);
      return swiftProperty(f, "    ", sent.has(d.name));
    });
    return `${swiftDocs(d.docs, "")}struct ${d.name}: Codable, Hashable {${fields.length ? `\n${fields.join("\n")}\n` : ""}}`;
  }
  if (d.kind !== "strings") fail(`Swift cannot read ${d.name}`);
  const fallback = d.variants.find((v) => v.other);
  const cases = d.variants
    .filter((v) => !v.other)
    .map((v) => {
      const name = swiftCase(v.name);
      const raw = name === v.name ? "" : ` = ${JSON.stringify(v.name)}`;
      return `${swiftDocs(v.docs, "    ")}    case ${name}${raw}`;
    });
  const unknown = fallback ? swiftCase(fallback.name) : "unknown";
  return `${swiftDocs(d.docs, "")}enum ${d.name}: String, Codable, Hashable {
${cases.join("\n")}
    /// One added after this build.
    case ${unknown}

    init(from decoder: Decoder) throws {
        self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .${unknown}
    }
}`;
}

/** A Swift property; one that is sent may leave out what is optional. */
function swiftProperty(f, indent, sending = false) {
  if (!/^[a-z]\w*$/.test(f.name)) fail(`${f.name} is no Swift name`);
  const type = swiftType(f.type);
  const optional = f.mayBeAbsent && !type.endsWith("?") ? "?" : "";
  const empty =
    { Bool: "false", Int: "0", Double: "0", String: '""' }[type] ??
    (type.startsWith("[") ? "[]" : null);
  const given =
    sending && f.defaulted && !optional && !type.endsWith("?") && empty
      ? ` = ${empty}`
      : "";
  const keyword =
    sending && (optional || type.endsWith("?") || given) ? "var" : "let";
  return `${swiftDocs(f.docs, indent)}${indent}${keyword} ${f.name}: ${type}${optional}${given}`;
}

/**
 * Swift for the notch. It decodes only the fields it reads, and a value the
 * core added after the notch was built reads as the enum's unknown case.
 */
function swift() {
  const declare = (d) => swiftDeclare(d, sent);
  const pick = (fields, key) => {
    const wanted = SWIFT_FIELDS[key]?.split(" ");
    if (!wanted) return fields;
    const missing = wanted.filter(
      (name) => !fields.some((f) => f.name === name),
    );
    if (missing.length) fail(`${key} has no ${missing.join(", ")}`);
    return fields.filter((field) => wanted.includes(field.name));
  };
  // A type named like one of the structs around it is named by its module there.
  const module = /Sources\/(\w+)\//.exec(OUTPUTS.notch)[1];
  const outside = (type, nested) =>
    nested.reduce(
      (text, name) =>
        text.replace(
          new RegExp(`(?<![.\\w])${name}\\b`, "g"),
          `${module}.${name}`,
        ),
      type,
    );

  const answered = askAnswers();
  const response = parse(byClientName("Response"), [
    ...new Set(SWIFT_REQUESTS.map((name) => answered.get(name))),
  ]);
  const variantOf = (rust) =>
    response.variants.find((v) => v.name === renameAll(rust, "camelCase")) ??
    fail(`Response has no ${rust}`);
  const askFile = `${WIRE}/protocol/ask.rs`;
  const requests = SWIFT_REQUESTS.map((name) => {
    const item = sources[SOURCES.indexOf(askFile)].get(name);
    const answer = answered.get(name) ?? fail(`ask::${name} has no answer`);
    if (answer === "Called")
      fail(`ask::${name} is a call; list its method in SWIFT_CALLS`);
    const fields = item ? parse(item).fields : [];
    return { name, docs: item?.docs ?? [], answer, fields };
  });
  const calls = coreCalls();
  const notchCalls = SWIFT_CALLS.map(
    (method) =>
      calls.find((call) => call.method === method) ??
      fail(`${method} names no params and answer in calls!`),
  );
  const event = parse(byClientName("Event"), SWIFT_EVENTS);
  const events = SWIFT_EVENTS.map((rust) => {
    const variant =
      event.variants.find((v) => v.name === renameAll(rust, "camelCase")) ??
      fail(`Event has no ${rust}`);
    return { ...variant, fields: pick(variant.fields, `Event.${rust}`) };
  });
  const answers = [...new Set(requests.map((request) => request.answer))].map(
    (rust) => ({
      rust,
      fields: pick(variantOf(rust).fields, `Response.${rust}`),
    }),
  );
  const used = [
    ...requests.flatMap((request) => request.fields),
    ...answers.flatMap((answer) => answer.fields),
    ...events.flatMap((variant) => variant.fields),
  ]
    .flatMap((field) => named(field.type))
    .concat(
      notchCalls.flatMap((call) => [
        ...named(call.takes),
        ...named(call.gives),
      ]),
    );

  const sent = new Set(
    closure(
      [
        ...requests.flatMap((request) => request.fields),
        ...notchCalls.map((call) => ({ type: call.takes })),
      ].flatMap((field) => named(field.type)),
      () => undefined,
      SWIFT_DECLARED,
    ).map((declaration) => declaration.name),
  );
  const pascal = (name) => name[0].toUpperCase() + name.slice(1);
  const callNames = SWIFT_CALLS.map(pascal);
  const asks = `/// A request the notch sends the core, sent with its \`op\`, and what the core answers it with.
protocol CoreAsk: Encodable {
    associatedtype Answer: Decodable
    static var op: String { get }
}

/// An answer that holds nothing, such as a call's \`()\`.
struct Nothing: Codable, Hashable {}

/// What the core answers the notch's requests with.
enum CoreResponse {
${answers
  .map(
    ({ rust, fields }) =>
      `    struct ${rust}: Decodable {${fields.length ? `\n${fields.map((f) => swiftProperty(f, "        ")).join("\n")}\n    ` : ""}}`,
  )
  .join("\n")}
    struct Called<Result: Decodable>: Decodable {
        let result: Result
    }
}

/// The requests the notch sends, as the core's \`ask\` module declares them.
enum CoreRequest {
${requests
  .map((request) =>
    `${swiftDocs(request.docs, "    ")}    struct ${request.name}: CoreAsk {
        static let op = ${JSON.stringify(renameAll(request.name, "camelCase"))}
        typealias Answer = CoreResponse.${request.answer}
${request.fields.map((f) => outside(swiftProperty(f, "        ", true), SWIFT_REQUESTS)).join("\n")}
    }`.replace(/\n\n {4}\}/, "\n    }"),
  )
  .join("\n\n")}
}

/// The core's calls the notch makes, each sent as a \`call\` request.
enum CoreCall {
${notchCalls
  .map((call) => {
    const empty =
      call.takes.kind === "named" &&
      parse(byClientName(call.takes.name)).fields?.length === 0;
    return `    struct ${pascal(call.method)}: CoreAsk {
        static let op = "call"
        typealias Answer = CoreResponse.Called<${outside(swiftType(call.gives), callNames)}>
        let method = ${JSON.stringify(call.method)}
        let params${empty ? ` = ${outside(swiftType(call.takes), callNames)}()` : `: ${outside(swiftType(call.takes), callNames)}`}
    }`;
  })
  .join("\n\n")}
}

${swiftDocs(event.docs, "")}enum Event: Decodable {
${events.map((v) => `${swiftDocs(v.docs, "    ")}    case ${v.name}${v.fields.length ? `(${v.fields.map((f) => `${f.name}: ${swiftType(f.type)}`).join(", ")})` : ""}`).join("\n")}
    /// One the notch does not read.
    case other

    private enum CodingKeys: String, CodingKey {
        case ${[event.tag, ...new Set(events.flatMap((v) => v.fields.map((f) => f.name)))].join(", ")}
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .${event.tag}) {
${events
  .map(
    (v) =>
      `        case ${JSON.stringify(v.name)}:\n            self = .${v.name}${v.fields.length ? `(${v.fields.map((f) => `${f.name}: try container.decode(${swiftType(f.type)}.self, forKey: .${f.name})`).join(", ")})` : ""}`,
  )
  .join("\n")}
        default:
            self = .other
        }
    }
}`;

  const number = (name) =>
    constant(`${WIRE}/protocol.rs`, name)
      .split("*")
      .reduce(
        (total, part) => total * Number(part.trim().replaceAll("_", "")),
        1,
      );
  return `// Generated by scripts/generate-wire-types.mjs from the core's Rust. Do not edit by hand.

import Foundation

/// The core's protocol as this build speaks it, so a core that no longer serves this version turns it away at hello.
enum WireProtocol {
    static let name = ${constant(`${WIRE}/protocol.rs`, "PROTOCOL")}
    static let version = ${number("PROTOCOL_VERSION")}
    /// The largest payload the core sends in one frame.
    static let maxFrameBytes = ${number("MAX_FRAME_BYTES")}
}

${asks}

${closure(
  [...SWIFT_ROOTS, ...used],
  (name) => SWIFT_FIELDS[name]?.split(" "),
  SWIFT_DECLARED,
)
  .map(declare)
  .join("\n\n")}
`;
}

const BANNER =
  "Generated by scripts/generate-wire-types.mjs from the core's Rust. Do not edit by hand.";

function cardSwift() {
  return `// ${BANNER}

import Foundation

${closure(CARD_ROOTS)
  .map((d) => swiftDeclare(d))
  .join("\n\n")}
`;
}

const kotlinDocs = (lines, indent) =>
  lines.length ? `${indent}/** ${lines.join(" ")} */\n` : "";
const kotlinCase = (name) =>
  words(name.replaceAll("-", "_")).join("_").toUpperCase();

/** The card in Kotlin, read from org.json, which the phone's Android code already uses. */
function cardKotlin() {
  const typeOf = (type) => {
    switch (type.kind) {
      case "string":
        return "String";
      case "bool":
        return "Boolean";
      case "int":
        return "Long";
      case "float":
        return "Double";
      case "named":
        return type.name;
      case "option":
        return `${typeOf(type.of)}?`;
      case "list":
        return `List<${typeOf(type.of)}>`;
      default:
        fail(`Kotlin cannot read ${type.kind}`);
    }
  };
  const declarations = closure(CARD_ROOTS);
  const kinds = new Map(declarations.map((d) => [d.name, d.kind]));
  const read = (type, key) => {
    switch (type.kind) {
      case "string":
        return `json.getString(${key})`;
      case "bool":
        return `json.getBoolean(${key})`;
      case "int":
        return `json.getLong(${key})`;
      case "float":
        return `json.getDouble(${key})`;
      case "named":
        if (kinds.get(type.name) !== "strings")
          fail(`Kotlin reads only strings enums, not ${type.name}`);
        return `${type.name}.of(json.getString(${key}))`;
      case "option":
        return `if (json.isNull(${key})) null else ${read(type.of, key)}`;
      case "list":
        if (type.of.kind !== "named" || kinds.get(type.of.name) !== "struct")
          fail(`Kotlin reads only lists of structs, not of ${type.of.kind}`);
        return `json.getJSONArray(${key}).let { list -> (0 until list.length()).mapNotNull { ${type.of.name}.of(list.getJSONObject(it)) } }`;
      default:
        fail(`Kotlin cannot read ${type.kind}`);
    }
  };
  const declare = (d) => {
    if (d.kind === "strings") {
      const cases = d.variants
        .filter((v) => !v.other)
        .map(
          (v) =>
            `${kotlinDocs(v.docs, "  ")}  ${kotlinCase(v.name)}(${JSON.stringify(v.name)}),`,
        );
      return `${kotlinDocs(d.docs, "")}enum class ${d.name}(val wire: String) {
${cases.join("\n")}
  /** One added after this build. */
  UNKNOWN("");

  companion object {
    fun of(wire: String): ${d.name} = entries.firstOrNull { it.wire == wire } ?: UNKNOWN
  }
}`;
    }
    if (d.kind !== "struct") fail(`Kotlin cannot read ${d.name}`);
    const fields = d.fields.map((f) => {
      const optional = f.mayBeAbsent && f.type.kind !== "option";
      const type = optional ? { kind: "option", of: f.type } : f.type;
      return { ...f, type };
    });
    return `${kotlinDocs(d.docs, "")}data class ${d.name}(
${fields.map((f) => `${kotlinDocs(f.docs, "  ")}  val ${f.name}: ${typeOf(f.type)},`).join("\n")}
) {
  companion object {
    /** Null when \`json\` is not one. */
    fun of(json: JSONObject): ${d.name}? =
      try {
        ${d.name}(
${fields.map((f) => `          ${f.name} = ${read(f.type, JSON.stringify(f.name))},`).join("\n")}
        )
      } catch (_: JSONException) {
        null
      }
  }
}`;
  };
  const pkg = /java\/(.+)\/[^/]+$/
    .exec(OUTPUTS.cardKotlin)[1]
    .replaceAll("/", ".");
  return `// ${BANNER}

package ${pkg}

import org.json.JSONException
import org.json.JSONObject

${declarations.map(declare).join("\n\n")}
`;
}

/**
 * Names Apple's frameworks, the app's own code and its Rust bridge (`SikemuxCore`) give their types, which a wire
 * type of the same name would hide in the app.
 */
const APPLE_NAMES = new Set(
  "Palette Theme Radius Screen Nav Row Rows Chip Track Field Sheet Router Route Hub Account Config Network Identity Keychain Format Haptics Device Connection DeviceIdentity ChatMark ChatAttachment CoreEvent CoreListener MobileError RelaySetting JoinAnswer Tab Layout Environment Text Image Color Font Data Date URL Group Section Label Event Link Menu Button Toggle Picker Path Shape Scene Window Alert Table List Form Grid Canvas State Binding Animation Transaction Namespace Edge Axis Angle Anchor Gradient Material Spacer Divider Notification Locale Calendar Decimal Bundle Timer Thread Progress Operation Measurement Unit Expression Predicate Result Never Task Error Duration Clock Range Set Character Optional Array Dictionary String Int Bool Double Float Mirror Unicode Process Host Stream Port Gauge Settings Badge".split(
    " ",
  ),
);

const SWIFT_KEYWORDS = new Set(
  "associatedtype class deinit enum extension fileprivate func import init inout internal let open operator private protocol public rethrows static struct subscript typealias var break case continue default defer do else fallthrough for guard if in repeat return switch where while as Any catch false is nil super self Self throw throws true try Type some any".split(
    " ",
  ),
);

const swiftIdent = (name) => (SWIFT_KEYWORDS.has(name) ? `\`${name}\`` : name);

const upperFirst = (name) => name[0].toUpperCase() + name.slice(1);

/** `AGENT_URI` as `agentURI`, for a Rust constant's name in Swift. */
const constantName = (name) =>
  name
    .toLowerCase()
    .split("_")
    .map((word, index) =>
      index === 0 ? word : word === "uri" ? "URI" : upperFirst(word),
    )
    .join("");

/**
 * Swift for the phone app: every type the TypeScript declares, whole, so it decodes all the
 * core sends and encodes all the phone sends. A field an older host may leave out is optional,
 * and a value or variant the core added after this build reads as the type's unknown case.
 */
async function phoneSwift() {
  const calls = coreCalls();
  const declarations = closure([
    ...TS_ROOTS,
    ...PHONE_ROOTS,
    ...calls.flatMap((call) => [...named(call.takes), ...named(call.gives)]),
  ]);
  const byName = new Map(declarations.map((d) => [d.name, d]));
  const typeName = (name) => (APPLE_NAMES.has(name) ? `Wire${name}` : name);
  // A variant's own struct is named inside its enum, so it must not hide a type the enum's code names.
  const taken = new Set([
    ...declarations.map((d) => typeName(d.name)),
    ..."String Int Bool Double JSONValue WireNull WirePair WireKey TagKey CodingKeys CodingKey DecodingError Decoder Encoder Self Type Protocol".split(
      " ",
    ),
  ]);

  const typeOf = (type) => {
    switch (type.kind) {
      case "string":
        return "String";
      case "bool":
        return "Bool";
      case "int":
        return "Int";
      case "float":
        return "Double";
      case "json":
        return "JSONValue";
      case "null":
        return "WireNull";
      case "named":
        return typeName(type.name);
      case "list":
        return `[${typeOf(type.of)}]`;
      case "map":
        return `[String: ${typeOf(type.of)}]`;
      case "option":
        if (type.of.kind === "option")
          fail("Swift cannot tell null from absent in Option<Option<_>>");
        return `${typeOf(type.of)}?`;
      case "tuple":
        if (type.of.length !== 2)
          fail(`Swift reads pairs, not ${type.of.length}-tuples`);
        return `WirePair<${type.of.map(typeOf).join(", ")}>`;
    }
  };

  const swiftName = (key, where) => {
    const name = /^[a-z][A-Za-z0-9]*$/.test(key)
      ? key
      : swiftCase(key.replace(/\(.*$/s, ""));
    if (!/^[a-z][A-Za-z0-9]*$/.test(name))
      fail(`${where}: ${key} is no Swift name`);
    return name;
  };
  const spare = (names) =>
    names.includes("unknown") ? "unrecognised" : "unknown";
  const unique = (names, where) => {
    const twice = names.find((name, index) => names.indexOf(name) !== index);
    if (twice) fail(`${where} has two ${twice} in Swift`);
  };

  const property = (f, where, indent) => {
    const type = typeOf(f.type);
    const optional =
      (f.mayBeAbsent || f.defaulted) && !type.endsWith("?") ? "?" : "";
    return `${swiftDocs(f.docs, indent)}${indent}var ${swiftIdent(swiftName(f.name, where))}: ${type}${optional}`;
  };
  const struct = (name, docs, fields, where, indent = "") => {
    unique(
      fields.map((f) => swiftName(f.name, where)),
      where,
    );
    const inner = `${indent}    `;
    const keys = fields.some((f) => swiftName(f.name, where) !== f.name)
      ? `\n\n${inner}private enum CodingKeys: String, CodingKey {\n${fields
          .map((f) => {
            const swift = swiftName(f.name, where);
            return `${inner}    case ${swiftIdent(swift)}${swift === f.name ? "" : ` = ${JSON.stringify(f.name)}`}`;
          })
          .join("\n")}\n${inner}}`
      : "";
    return `${swiftDocs(docs, indent)}${indent}struct ${name}: Codable, Hashable, Sendable {${
      fields.length
        ? `\n${fields.map((f) => property(f, where, inner)).join("\n")}${keys}\n${indent}`
        : ""
    }}`;
  };

  const held = (d) => {
    switch (d.kind) {
      case "struct":
        return d.fields.map((f) => f.type);
      case "tagged":
        return d.variants.flatMap((v) => v.fields.map((f) => f.type));
      case "wrapped":
        return d.variants.map((v) => v.type);
      case "untagged":
        return d.variants.flatMap((v) =>
          v.fields ? v.fields.map((f) => f.type) : [v.type],
        );
      default:
        return [];
    }
  };
  // Lists and maps hold their values apart already; anything else held inline that holds its holder needs a box.
  const inline = (type) =>
    type.kind === "named"
      ? [type.name]
      : type.kind === "option"
        ? inline(type.of)
        : type.kind === "tuple"
          ? type.of.flatMap(inline)
          : [];
  const reaches = (from, to, seen = new Set()) =>
    held(byName.get(from))
      .flatMap(inline)
      .some((name) => {
        if (name === to) return true;
        if (seen.has(name)) return false;
        seen.add(name);
        return reaches(name, to, seen);
      });
  const cyclic = new Set(
    declarations.filter((d) => reaches(d.name, d.name)).map((d) => d.name),
  );
  for (const name of cyclic)
    if (
      byName.get(name).kind === "struct" &&
      ![...cyclic].some(
        (other) =>
          byName.get(other).kind !== "struct" &&
          reaches(name, other) &&
          reaches(other, name),
      )
    )
      fail(`${name} holds itself with no enum between to box it`);
  const indirect = (d) => (cyclic.has(d.name) ? "indirect " : "");

  const payloadName = (variant, where) => {
    const name = upperFirst(variant);
    const free = taken.has(name) ? `${name}Payload` : name;
    if (taken.has(free)) fail(`${where} has no free Swift name for ${variant}`);
    return free;
  };

  const strings = (d) => {
    const name = typeName(d.name);
    const where = d.name;
    const open = d.variants.find((v) => v.open);
    const fallback = d.variants.find((v) => v.other && !v.open);
    const known = d.variants
      .filter((v) => !v.other)
      .map((v) => ({ ...v, swift: swiftName(v.name, where) }));
    const rest = open
      ? swiftName(open.name, where)
      : fallback
        ? swiftName(fallback.name, where)
        : spare(known.map((v) => v.swift));
    unique([...known.map((v) => v.swift), rest], where);
    const cases = known.map(
      (v) =>
        `${swiftDocs(v.docs, "    ")}    case ${swiftIdent(v.swift)}${!open && v.swift !== v.name ? ` = ${JSON.stringify(v.name)}` : ""}`,
    );
    if (!open) {
      const raw =
        fallback && rest !== fallback.name
          ? ` = ${JSON.stringify(fallback.name)}`
          : "";
      return `${swiftDocs(d.docs, "")}enum ${name}: String, Codable, Hashable, Sendable {
${cases.join("\n")}
    /// One added after this build.
    case ${swiftIdent(rest)}${raw}

    init(from decoder: any Decoder) throws {
        self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .${swiftIdent(rest)}
    }
}`;
    }
    return `${swiftDocs(d.docs, "")}enum ${name}: Codable, Hashable, Sendable {
${cases.join("\n")}
${swiftDocs(open.docs, "    ")}    case ${swiftIdent(rest)}(String)

    init(rawValue: String) {
        switch rawValue {
${known.map((v) => `        case ${JSON.stringify(v.name)}: self = .${swiftIdent(v.swift)}`).join("\n")}
        default: self = .${swiftIdent(rest)}(rawValue)
        }
    }

    var rawValue: String {
        switch self {
${known.map((v) => `        case .${swiftIdent(v.swift)}: return ${JSON.stringify(v.name)}`).join("\n")}
        case .${swiftIdent(rest)}(let value): return value
        }
    }

    init(from decoder: any Decoder) throws {
        self.init(rawValue: try decoder.singleValueContainer().decode(String.self))
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}`;
  };

  const tagged = (d) => {
    const name = typeName(d.name);
    const variants = d.variants.map((v) => {
      const swift = swiftName(v.name, d.name);
      return {
        ...v,
        swift,
        payload: v.fields.length ? payloadName(swift, d.name) : null,
      };
    });
    const rest = spare(variants.map((v) => v.swift));
    unique(
      variants.filter((v) => v.payload).map((v) => v.payload),
      d.name,
    );
    const payloads = variants
      .filter((v) => v.payload)
      .map((v) =>
        struct(v.payload, [], v.fields, `${d.name}::${v.name}`, "    "),
      );
    return `${swiftDocs(d.docs, "")}${indirect(d)}enum ${name}: Codable, Hashable, Sendable {
${variants.map((v) => `${swiftDocs(v.docs, "    ")}    case ${swiftIdent(v.swift)}${v.payload ? `(${v.payload})` : ""}`).join("\n")}
    /// One added after this build, by its \`${d.tag}\`.
    case ${rest}(String)
${payloads.length ? `\n${payloads.join("\n\n")}\n` : ""}
    private enum TagKey: String, CodingKey {
        case tag = ${JSON.stringify(d.tag)}
    }

    init(from decoder: any Decoder) throws {
        let tag = try decoder.container(keyedBy: TagKey.self).decode(String.self, forKey: .tag)
        switch tag {
${variants
  .map(
    (v) =>
      `        case ${JSON.stringify(v.name)}:\n            self = .${swiftIdent(v.swift)}${v.payload ? `(try ${v.payload}(from: decoder))` : ""}`,
  )
  .join("\n")}
        default:
            self = .${rest}(tag)
        }
    }

    func encode(to encoder: any Encoder) throws {
        let tag: String
        switch self {
${variants
  .map((v) =>
    v.payload
      ? `        case .${swiftIdent(v.swift)}(let value):\n            try value.encode(to: encoder)\n            tag = ${JSON.stringify(v.name)}`
      : `        case .${swiftIdent(v.swift)}:\n            tag = ${JSON.stringify(v.name)}`,
  )
  .join("\n")}
        case .${rest}(let value):
            tag = value
        }
        var container = encoder.container(keyedBy: TagKey.self)
        try container.encode(tag, forKey: .tag)
    }
}`;
  };

  const wrapped = (d) => {
    const name = typeName(d.name);
    const variants = d.variants.map((v) => ({
      ...v,
      swift: swiftName(v.name, d.name),
    }));
    const rest = spare(variants.map((v) => v.swift));
    return `${swiftDocs(d.docs, "")}${indirect(d)}enum ${name}: Codable, Hashable, Sendable {
${variants.map((v) => `${swiftDocs(v.docs, "    ")}    case ${swiftIdent(v.swift)}(${typeOf(v.type)})`).join("\n")}
    /// One added after this build, by its key.
    case ${rest}(String)

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: WireKey.self)
        guard let key = container.allKeys.first else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "${d.name} names no variant"))
        }
        switch key.stringValue {
${variants
  .map(
    (v) =>
      `        case ${JSON.stringify(v.name)}:\n            self = .${swiftIdent(v.swift)}(try container.decode(${typeOf(v.type)}.self, forKey: key))`,
  )
  .join("\n")}
        default:
            self = .${rest}(key.stringValue)
        }
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: WireKey.self)
        switch self {
${variants
  .map(
    (v) =>
      `        case .${swiftIdent(v.swift)}(let value):\n            try container.encode(value, forKey: WireKey(${JSON.stringify(v.name)}))`,
  )
  .join("\n")}
        case .${rest}(let key):
            try container.encode(JSONValue.null, forKey: WireKey(key))
        }
    }
}`;
  };

  const untagged = (d) => {
    const name = typeName(d.name);
    const variants = d.variants.map((v) => {
      const swift = swiftName(v.name, d.name);
      const payload = v.fields ? payloadName(swift, d.name) : null;
      return { ...v, swift, payload, type: payload ?? typeOf(v.type) };
    });
    unique(
      variants.map((v) => v.swift),
      d.name,
    );
    const payloads = variants
      .filter((v) => v.payload)
      .map((v) =>
        struct(v.payload, [], v.fields, `${d.name}::${v.name}`, "    "),
      );
    return `${swiftDocs(d.docs, "")}${indirect(d)}enum ${name}: Codable, Hashable, Sendable {
${variants.map((v) => `${swiftDocs(v.docs, "    ")}    case ${swiftIdent(v.swift)}(${v.type})`).join("\n")}
${payloads.length ? `\n${payloads.join("\n\n")}\n` : ""}
    init(from decoder: any Decoder) throws {
${variants
  .map(
    (v) =>
      `        if let value = try? ${v.type}(from: decoder) {\n            self = .${swiftIdent(v.swift)}(value)\n            return\n        }`,
  )
  .join("\n")}
        throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "${d.name} matches no variant"))
    }

    func encode(to encoder: any Encoder) throws {
        switch self {
${variants.map((v) => `        case .${swiftIdent(v.swift)}(let value):\n            try value.encode(to: encoder)`).join("\n")}
        }
    }
}`;
  };

  const declare = (d) =>
    d.kind === "struct"
      ? struct(typeName(d.name), d.docs, d.fields, d.name)
      : d.kind === "strings"
        ? strings(d)
        : d.kind === "tagged"
          ? tagged(d)
          : d.kind === "wrapped"
            ? wrapped(d)
            : untagged(d);

  const callNames = calls.map((call) => upperFirst(call.method));
  unique(callNames, "PhoneCall");
  const callTable = `/// A call the phone makes: the core's method, what it takes and what it answers.
protocol PhoneCallable {
    associatedtype Params: Encodable & Sendable
    associatedtype Answer: Decodable & Sendable
    static var method: String { get }
}

/// The core's calls the phone makes, as mobile/app/wire.json names them.
enum PhoneCall {
${callNames.map((name) => `    enum ${name} {}`).join("\n")}
}

${calls
  .map(
    (call, index) => `extension PhoneCall.${callNames[index]}: PhoneCallable {
    static let method = ${JSON.stringify(call.method)}
    typealias Params = ${typeOf(call.takes)}
    typealias Answer = ${typeOf(call.gives)}
}`,
  )
  .join("\n\n")}`;

  const number = (name) =>
    constant(`${WIRE}/protocol.rs`, name)
      .split("*")
      .reduce(
        (total, part) => total * Number(part.trim().replaceAll("_", "")),
        1,
      );
  const extensionless = [
    ...(await extensionlessNames()).matchAll(/"[^"]*"/g),
  ].map(([name]) => name);

  const body = declarations.map(declare).join("\n\n");
  const glance = closure(GLANCE_ROOTS)
    .map((d) =>
      declare(
        byName.get(d.name) ?? fail(`${d.name} is not in the phone's view`),
      ),
    )
    .join("\n\n");
  glanceSwift = `// ${BANNER}

import Foundation

${glance}
`;
  return `// ${BANNER}

import Foundation

/// The core's protocol as this build speaks it, so a core that no longer serves this version turns it away at hello.
enum WireProtocol {
    static let name = ${constant(`${WIRE}/protocol.rs`, "PROTOCOL")}
    static let version = ${number("PROTOCOL_VERSION")}
    /// The largest payload the core sends in one frame.
    static let maxFrameBytes = ${number("MAX_FRAME_BYTES")}
}

/// Limits the core enforces on what the phone sends, so the phone checks them first.
enum WireLimits {
${limitValues.map(([name, value]) => `    static let ${constantName(name)} = ${value}`).join("\n")}
}

/// How long the host waits on the phone, in seconds, so the phone waits as long.
enum WireWaits {
${waitValues.map(([name, value]) => `    static let ${constantName(name)} = ${value}`).join("\n")}
}

/// Addresses the phone writes into what it sends, as the core reads them.
enum WireURIs {
${(await uriValues()).map(([name, value]) => `    static let ${constantName(name)} = ${value}`).join("\n")}
}

/// Paths as the core reads them in markdown.
enum WirePaths {
    /// The bare names a project spells out in full, which are files without an extension.
    static let extensionless: [String] = [${extensionless.join(", ")}]
}

/// Any JSON, for what the core sends as it is.
enum JSONValue: Codable, Hashable, Sendable {
    case null
    case bool(Bool)
    case int(Int)
    case double(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Int.self) {
            self = .int(value)
        } else if let value = try? container.decode(Double.self) {
            self = .double(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .int(let value): try container.encode(value)
        case .double(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

/// What a call that answers nothing answers: JSON's null.
struct WireNull: Codable, Hashable, Sendable {
    init() {}

    init(from decoder: any Decoder) throws {}

    func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encodeNil()
    }
}

/// A Rust pair, which JSON holds as an array of two.
struct WirePair<First: Codable & Hashable & Sendable, Second: Codable & Hashable & Sendable>: Codable, Hashable, Sendable {
    var first: First
    var second: Second

    init(_ first: First, _ second: Second) {
        self.first = first
        self.second = second
    }

    init(from decoder: any Decoder) throws {
        var container = try decoder.unkeyedContainer()
        first = try container.decode(First.self)
        second = try container.decode(Second.self)
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.unkeyedContainer()
        try container.encode(first)
        try container.encode(second)
    }
}

/// A key read from the JSON itself, for an enum keyed by its variant.
struct WireKey: CodingKey {
    let stringValue: String
    var intValue: Int? { nil }

    init(_ stringValue: String) {
        self.stringValue = stringValue
    }

    init?(stringValue: String) {
        self.stringValue = stringValue
    }

    init?(intValue: Int) {
        nil
    }
}

${callTable}

${body}
`;
}

/** The kind of Response each request in the core's `ask` module is answered with, by request. */
function askAnswers() {
  const text = texts[SOURCES.indexOf(`${WIRE}/protocol/ask.rs`)];
  const block = (name) => {
    const at = text.indexOf(`\n${name}! {`);
    if (at < 0) fail(`ask.rs has no ${name}! table`);
    const open = text.indexOf("{", at);
    return text.slice(open + 1, balanced(text, open) - 1);
  };
  const answers = new Map();
  for (const name of splitTop(block("done"))) answers.set(name, "Done");
  for (const [, request, response] of block("answers").matchAll(
    /^\s*(\w+)\s*->.*=>\s*(\w+)\s*\{/gm,
  ))
    answers.set(request, response);
  return answers;
}

/** A constant's value, `const NAME: T = value;` in `file`. */
function constant(file, name) {
  const text = texts[SOURCES.indexOf(file)];
  const value = new RegExp(
    `^(?:pub )?const ${name}:[^=]+=\\s*([\\s\\S]*?);$`,
    "m",
  ).exec(text)?.[1];
  if (value === undefined) fail(`${file} has no constant ${name}`);
  return value;
}

async function emit(path, contents) {
  const full = resolve(root, path);
  if (path.endsWith(".ts"))
    contents = await format(contents, {
      ...(await resolveConfig(full)),
      filepath: full,
    });
  let actual = null;
  try {
    actual = await readFile(full, "utf8");
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  if (actual === contents) return;
  if (check) fail(`${path} is stale: run pnpm wire:generate`);
  await mkdir(dirname(full), { recursive: true });
  await writeFile(full, contents);
}

// A gate's id in the core's gate table is the name the wire gives it.
const gateTable = await readFile(
  resolve(root, `${CORE}/server/board/slice/ladder.rs`),
  "utf8",
);
const gateKeys = [
  ...gateTable.matchAll(/Gate::(\w+)\) => Rule \{[^}]*?key: "(\w+)"/gu),
];
if (gateKeys.length === 0)
  fail("the gate table in board/slice/ladder.rs names no gate's key");
for (const [, variant, key] of gateKeys) {
  const wire = variant[0].toLowerCase() + variant.slice(1);
  if (key !== wire)
    fail(
      `the gate table in board/slice/ladder.rs gives Gate::${variant} the id "${key}", but the wire calls it "${wire}"`,
    );
}

await emit(
  OUTPUTS.ts,
  `${typescript(TS_ROOTS, `${limits}\n${waits}\n${await uris()}\n${await extensionless()}\n\n`)}\n`,
);
await emit(OUTPUTS.phone, await phoneSwift());
await emit(OUTPUTS.notch, swift());
await emit(OUTPUTS.cardSwift, cardSwift());
await emit(OUTPUTS.glanceSwift, glanceSwift);
await emit(OUTPUTS.cardKotlin, cardKotlin());
