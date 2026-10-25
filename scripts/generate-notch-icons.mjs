#!/usr/bin/env node
// Writes the notch helper's icons from the app's own: the line icons in
// shared/ui/Icons.tsx and the agent marks beside them. `--check` fails when the
// Swift file is out of date instead of writing it.

import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const iconsPath = resolve(root, "shared/ui/Icons.tsx");
const logosDir = resolve(root, "shared/assets/agent-logos");
const outputPath = resolve(
  root,
  "src-tauri/notch/Sources/NotchKit/Icons.generated.swift",
);
const check = process.argv.includes("--check");

const LINE_ICONS = {
  plus: "IconPlus",
  phone: "IconPhone",
  folder: "IconFolder",
  file: "IconFile",
  shield: "IconShield",
  shieldBolt: "IconShieldBolt",
  chevron: "IconChevron",
  check: "IconCheck",
  arrowUp: "IconArrowUp",
  agent: "IconAgent",
  command: "IconCommand",
};

// Line icons the app fills from its stylesheet rather than from the icon itself.
const FILLED_ICONS = new Set(["agent"]);

const MARK_FUNCTIONS = {
  claude: "IconClaude",
  pi: "IconPi",
  opencode: "IconOpenCode",
  grok: "IconGrok",
  hermes: "IconHermes",
};

const MARK_FILES = { codex: "codex.svg", omp: "omp.svg", muse: "muse.svg" };
const BRAND_MARK = resolve(root, "brand/mark/mark-white.svg");

/** The colour each mark takes where the app paints it with its brand colour. */
const BRAND = Object.fromEntries(
  JSON.parse(
    await readFile(
      resolve(
        root,
        "src-tauri/crates/sikemux-core/src/assets/agent-catalog.json",
      ),
      "utf8",
    ),
  ).agents.map((agent) => [agent.type, agent.brand]),
);

function attributes(text) {
  const found = {};
  for (const [, name, quoted, braced] of text.matchAll(
    /([a-zA-Z0-9-]+)=(?:"([^"]*)"|\{([^}]*)\})/g,
  )) {
    found[name] = quoted ?? braced.replace(/^"|"$/g, "");
  }
  return found;
}

function number(value, fallback = 0) {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

/** Arithmetic on the attributes leaves float noise such as 6.199999999999999. */
function fixed(value) {
  return Number(value.toFixed(4));
}

function rectPath({ x, y, width, height, rx }) {
  const [left, top, w, h, r] = [x, y, width, height, rx].map((value) =>
    number(value),
  );
  const f = fixed;
  if (!r) return `M${f(left)} ${f(top)}h${f(w)}v${f(h)}h${f(-w)}z`;
  return (
    `M${f(left + r)} ${f(top)}h${f(w - 2 * r)}a${r} ${r} 0 0 1 ${r} ${r}v${f(h - 2 * r)}` +
    `a${r} ${r} 0 0 1 ${-r} ${r}h${f(-(w - 2 * r))}a${r} ${r} 0 0 1 ${-r} ${-r}` +
    `v${f(-(h - 2 * r))}a${r} ${r} 0 0 1 ${r} ${-r}z`
  );
}

function circlePath({ cx, cy, r }) {
  const [x, y, radius] = [cx, cy, r].map((value) => number(value));
  return `M${fixed(x - radius)} ${y}a${radius} ${radius} 0 1 0 ${fixed(2 * radius)} 0a${radius} ${radius} 0 1 0 ${fixed(-2 * radius)} 0z`;
}

/** Every shape in a piece of SVG or JSX, with the translate of the group it sits in. */
function shapes(markup) {
  const found = [];
  const groups = [];
  for (const [tag, closing, name, rest] of markup.matchAll(
    /<(\/?)(g|path|rect|circle|polygon)\b([^>]*?)\/?>/g,
  )) {
    if (name === "g") {
      if (closing) groups.pop();
      else if (!tag.endsWith("/>")) groups.push(attributes(rest));
      continue;
    }
    const own = attributes(rest);
    const inherited = Object.assign({}, ...groups);
    const all = { ...inherited, ...own };
    const translate = groups
      .map((group) =>
        group.transform?.match(/translate\(\s*([-\d.]+)[ ,]+([-\d.]+)\s*\)/),
      )
      .filter(Boolean)
      .reduce(
        ([x, y], match) => [x + number(match[1]), y + number(match[2])],
        [0, 0],
      );
    const d =
      name === "path"
        ? all.d
        : name === "rect"
          ? rectPath(all)
          : name === "polygon"
            ? `M${all.points.trim().replace(/\s+/g, " L")}Z`
            : circlePath(all);
    found.push({
      d,
      fill: all.fill,
      stroke: all.stroke,
      opacity: all.opacity ? number(all.opacity, 1) : 1,
      evenOdd: (all.fillRule ?? all["fill-rule"]) === "evenodd",
      translate,
    });
  }
  return found;
}

function swiftString(text) {
  return JSON.stringify(text);
}

function viewBox(text) {
  const values = (text ?? "0 0 16 16")
    .trim()
    .split(/[\s,]+/)
    .map((value) => number(value));
  return `ViewBox(x: ${values[0]}, y: ${values[1]}, width: ${values[2]}, height: ${values[3]})`;
}

function oklchToHex(text) {
  const [l, c, h] = text
    .match(/oklch\(([^)]+)\)/)[1]
    .trim()
    .split(/\s+/)
    .map(Number);
  const radians = (h * Math.PI) / 180;
  const a = c * Math.cos(radians);
  const b = c * Math.sin(radians);
  const lp = l + 0.3963377774 * a + 0.2158037573 * b;
  const mp = l - 0.1055613458 * a - 0.0638541728 * b;
  const sp = l - 0.0894841775 * a - 1.291485548 * b;
  const [L, M, S] = [lp ** 3, mp ** 3, sp ** 3];
  const linear = [
    4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S,
    -1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S,
    -0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S,
  ];
  const gamma = (value) => {
    const clamped = Math.min(1, Math.max(0, value));
    return clamped <= 0.0031308
      ? 12.92 * clamped
      : 1.055 * clamped ** (1 / 2.4) - 0.055;
  };
  return `#${linear
    .map((value) =>
      Math.round(gamma(value) * 255)
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}

function colour(text) {
  if (text.startsWith("oklch")) return oklchToHex(text);
  return text;
}

function fraction(value, fallback = 0) {
  return value?.endsWith("%")
    ? fixed(number(value) / 100)
    : number(value, fallback);
}

function gradients(svg) {
  const found = {};
  for (const [, open, body] of svg.matchAll(
    /<linearGradient\b([^>]*)>([\s\S]*?)<\/linearGradient>/g,
  )) {
    const own = attributes(open);
    const stops = [...body.matchAll(/<stop\b([^>]*)\/>/g)].map(([, stop]) => {
      const { offset, "stop-color": stopColour } = attributes(stop);
      return { offset: fraction(offset), colour: colour(stopColour) };
    });
    const turned = own.gradientTransform?.startsWith("matrix(0");
    const start = turned ? [0.5, 0] : [fraction(own.x1), fraction(own.y1)];
    const end = turned ? [0.5, 1] : [fraction(own.x2, 1), fraction(own.y2)];
    found[own.id] = { stops, start, end };
  }
  return found;
}

function paint(fill, brand, svgGradients) {
  if (fill?.startsWith("url(#")) {
    const gradient = svgGradients[fill.slice(5, -1)];
    const stops = gradient.stops
      .map(
        (stop) =>
          `.init(offset: ${stop.offset}, hex: ${swiftString(stop.colour)})`,
      )
      .join(", ");
    return `.gradient([${stops}], start: (${gradient.start.join(", ")}), end: (${gradient.end.join(", ")}))`;
  }
  if (fill && fill !== "currentColor" && fill !== "none")
    return `.hex(${swiftString(colour(fill))})`;
  return brand ? `.hex(${swiftString(brand)})` : ".current";
}

function shapeLiteral(shape, style, brand, svgGradients) {
  const filled =
    style === "fill" ||
    (shape.fill && shape.fill !== "none" && shape.stroke === "none");
  const kind = filled
    ? `.fill(${paint(shape.fill, brand, svgGradients)})`
    : ".stroke";
  return (
    `IconShape(d: ${swiftString(shape.d)}, style: ${kind}, opacity: ${shape.opacity}, ` +
    `evenOdd: ${shape.evenOdd}, translate: (${shape.translate.join(", ")}))`
  );
}

const source = await readFile(iconsPath, "utf8");
const lines = [];

for (const [key, name] of Object.entries(LINE_ICONS)) {
  const match = source.match(
    new RegExp(
      `export const ${name} = makeSvgIcon\\(\\s*([\\s\\S]*?)\\s*(?:,\\s*(\\{[^{}]*\\}))?\\s*,?\\s*\\);\\n`,
    ),
  );
  if (!match) throw new Error(`shared/ui/Icons.tsx has no ${name}`);
  const options = match[2] ?? "";
  const box = options.match(/viewBox:\s*"([^"]+)"/)?.[1];
  const style =
    FILLED_ICONS.has(key) || options.includes('fill: "currentColor"')
      ? "fill"
      : "stroke";
  const body = shapes(match[1]).map(
    (shape) => `        ${shapeLiteral(shape, style, null, {})},`,
  );
  lines.push(
    `    static let ${key} = IconDef(viewBox: ${viewBox(box)}, shapes: [\n${body.join("\n")}\n    ])`,
  );
}

const marks = [];
for (const [key, name] of Object.entries(MARK_FUNCTIONS)) {
  const match = source.match(
    new RegExp(
      `export function ${name}\\b[\\s\\S]*?return \\(([\\s\\S]*?)\\);\\n}`,
    ),
  );
  if (!match) throw new Error(`shared/ui/Icons.tsx has no ${name}`);
  const box = match[1].match(/viewBox="([^"]+)"/)?.[1];
  const body = shapes(match[1]).map(
    (shape) => `        ${shapeLiteral(shape, "fill", BRAND[key], {})},`,
  );
  marks.push(
    `        case ${swiftString(key)}: return IconDef(viewBox: ${viewBox(box)}, shapes: [\n${body.join("\n")}\n        ])`,
  );
}
for (const [key, file] of Object.entries(MARK_FILES)) {
  const svg = await readFile(resolve(logosDir, file), "utf8");
  const box = svg.match(/viewBox="([^"]+)"/)?.[1];
  const svgGradients = gradients(svg);
  const body = shapes(svg.replace(/<defs>[\s\S]*?<\/defs>/, "")).map(
    (shape) => `        ${shapeLiteral(shape, "fill", null, svgGradients)},`,
  );
  marks.push(
    `        case ${swiftString(key)}: return IconDef(viewBox: ${viewBox(box)}, shapes: [\n${body.join("\n")}\n        ])`,
  );
}

const brand = await readFile(BRAND_MARK, "utf8");
const brandBody = shapes(brand).map(
  (shape) =>
    `        IconShape(d: ${swiftString(shape.d)}, style: .fill(.current), opacity: 1, evenOdd: false, translate: (0, 0)),`,
);
lines.push(
  `    static let brand = IconDef(viewBox: ${viewBox(brand.match(/viewBox="([^"]+)"/)?.[1])}, shapes: [\n${brandBody.join("\n")}\n    ])`,
);

const swift = `// Generated by scripts/generate-notch-icons.mjs from shared/ui/Icons.tsx and
// shared/assets/agent-logos. Do not edit by hand.

enum Icons {
${lines.join("\n")}

    /// The mark of the agent \`provider\`, or nil for one the app has no mark for.
    static func mark(_ provider: String) -> IconDef? {
        switch provider {
${marks.join("\n")}
        default: return nil
        }
    }
}
`;

if (check) {
  const current = await readFile(outputPath, "utf8").catch(() => "");
  if (current !== swift) {
    console.error(
      "The notch icons are out of date. Run: node scripts/generate-notch-icons.mjs",
    );
    process.exit(1);
  }
} else {
  await writeFile(outputPath, swift);
}
