#!/usr/bin/env node
// Writes the notch helper's icons, which the phone's share extension draws too,
// from the app's own: the line icons in shared/ui/Icons.tsx and the agent marks
// beside them. The iPhone app builds the notch's file and adds the rest of the
// icons it draws from a second one. `--check` fails when either Swift file is out
// of date instead of writing it.

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { emit, typescriptLoader } from "./lib/generated.mjs";

const root = resolve(import.meta.dirname, "..");
const iconsPath = resolve(root, "shared/ui/Icons.tsx");
const logosDir = resolve(root, "shared/assets/agent-logos");
const outputPath = resolve(
  root,
  "helpers/notch/Sources/NotchKit/Icons.generated.swift",
);
const phoneOutputPath = resolve(
  root,
  "mobile/ios/Sikemux/UI/PhoneIcons.generated.swift",
);

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
  stack: "IconStack",
  globe: "IconGlobe",
  timer: "IconTimer",
  server: "IconServer",
  pencil: "IconPencil",
  clock: "IconClock",
  message: "IconMessage",
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
      resolve(root, "crates/sikemux-core/src/assets/agent-catalog.json"),
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
    if (closing) continue;
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
      transform: own.transform,
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
    found[own.id] = {
      stops,
      start,
      end,
      userSpace: own.gradientUnits === "userSpaceOnUse",
    };
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
  const match =
    source.match(
      new RegExp(
        `export const ${name} = makeSvgIcon\\(\\s*([\\s\\S]*?)\\s*(?:,\\s*(\\{[^{}]*\\}))?\\s*,?\\s*\\);\\n`,
      ),
    ) ??
    // An icon drawn on a 24-unit grid, which makeSvgIcon24 gives its own viewBox.
    source
      .match(
        new RegExp(
          `export const ${name} = makeSvgIcon24\\(([\\s\\S]*?)\\);\\n`,
        ),
      )
      ?.slice(0, 2)
      .concat('{ viewBox: "0 0 24 24" }');
  if (!match) throw new Error(`shared/ui/Icons.tsx has no ${name}`);
  const options = match[2] ?? "";
  const box = options.match(/viewBox:\s*"([^"]+)"/)?.[1];
  const style =
    FILLED_ICONS.has(key) || options.includes('fill: "currentColor"')
      ? "fill"
      : "stroke";
  // makeSvgIcon24 draws at 2.1 on its own grid, where the rest draw at 1.4 on 16 units.
  const grid24 = box === "0 0 24 24";
  const body = shapes(match[1]).map((shape) => {
    const literal = shapeLiteral(shape, style, null, {});
    return `        ${grid24 ? `${literal.slice(0, -1)}, lineWidth: 2.1)` : literal},`;
  });
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

await emit(outputPath, swift);

/** The icons the phone draws, by the name shared/ui/Icons.tsx exports each under. */
const PHONE_ICONS = [
  "Logo",
  "IconActivity",
  "IconAgent",
  "IconAgents",
  "IconArrowDown",
  "IconArrowUp",
  "IconAws",
  "IconBoard",
  "IconBook",
  "IconBolt",
  "IconCamera",
  "IconCheck",
  "IconChevron",
  "IconClaude",
  "IconClick",
  "IconClock",
  "IconClose",
  "IconCode",
  "IconCodex",
  "IconCommit",
  "IconCommand",
  "IconCopy",
  "IconCube",
  "IconDashboard",
  "IconDownload",
  "IconEye",
  "IconFile",
  "IconFilePlus",
  "IconFolder",
  "IconFlask",
  "IconGit",
  "IconGlobe",
  "IconGrok",
  "IconHermes",
  "IconHighlighter",
  "IconHomeButton",
  "IconHourglass",
  "IconImage",
  "IconInbox",
  "IconInfo",
  "IconIssue",
  "IconKeyboard",
  "IconLink",
  "IconLock",
  "IconLogLines",
  "IconLogOut",
  "IconMessage",
  "IconMore",
  "IconMoreVertical",
  "IconMouse",
  "IconMove",
  "IconMuse",
  "IconNetwork",
  "IconOmp",
  "IconOpenCode",
  "IconPanel",
  "IconPencil",
  "IconPhone",
  "IconPi",
  "IconPinch",
  "IconPlug",
  "IconPlus",
  "IconPointer",
  "IconPower",
  "IconPullRequest",
  "IconPush",
  "IconRecord",
  "IconRefresh",
  "IconResize",
  "IconRocket",
  "IconRotate",
  "IconRun",
  "IconRundeck",
  "IconSearch",
  "IconServer",
  "IconShare",
  "IconShield",
  "IconShieldBolt",
  "IconSquarePlus",
  "IconStack",
  "IconStatus",
  "IconSteps",
  "IconStop",
  "IconSwipe",
  "IconTabs",
  "IconTap",
  "IconTerminal",
  "IconText",
  "IconTimer",
  "IconTouchPath",
  "IconTrash",
  "IconUnlink",
  "IconUpload",
  "IconUser",
  "IconWarning",
  "IconWaterfall",
  "IconWindow",
  "IconWorkflow",
  "IconWrench",
];

/** Line icons the phone also paints solid, as the `filled` of its RN `Icon` did. */
const PHONE_FILLED = new Set(["IconAgent"]);

const DRAWN_ICONS_PATH = "/shared/ui/drawnIcons.ts";

const MARK_NAMES = {
  ...Object.fromEntries(
    Object.entries(MARK_FUNCTIONS).map(([key, name]) => [name, key]),
  ),
  IconCodex: "codex",
  IconOmp: "omp",
  IconMuse: "muse",
};

const SHAPE_PROPS = new Set([
  "d",
  "x",
  "y",
  "width",
  "height",
  "rx",
  "ry",
  "cx",
  "cy",
  "r",
  "x1",
  "y1",
  "x2",
  "y2",
  "points",
]);
const PAINT_PROPS = new Set([
  "fill",
  "stroke",
  "strokeWidth",
  "strokeDasharray",
  "strokeLinecap",
  "strokeLinejoin",
  "fillRule",
  "clipRule",
  "opacity",
  "transform",
]);
const IGNORED_PROPS = new Set([
  "children",
  "className",
  "aria-hidden",
  "xmlns",
]);

function outline(type, props) {
  const n = (key) => number(props[key]);
  switch (type) {
    case "path":
      return props.d.trim().replace(/\s+/g, " ");
    case "rect":
      return rectPath({ ...props, rx: props.rx ?? props.ry });
    case "circle":
    case "ellipse": {
      const [cx, cy] = [n("cx"), n("cy")];
      const [rx, ry] =
        type === "circle" ? [n("r"), n("r")] : [n("rx"), n("ry")];
      return `M${fixed(cx + rx)} ${cy}a${rx} ${ry} 0 1 1 ${fixed(-2 * rx)} 0a${rx} ${ry} 0 1 1 ${fixed(2 * rx)} 0z`;
    }
    case "line":
      return `M${n("x1")} ${n("y1")}L${n("x2")} ${n("y2")}`;
    case "polyline":
    case "polygon":
      return `M${props.points.trim().replace(/\s+/g, " L")}${type === "polygon" ? "Z" : ""}`;
    default:
      throw new Error(`the phone's icons cannot draw <${type}>`);
  }
}

/**
 * Every shape an icon's React element draws, with the paint it inherits from
 * the groups and the <svg> around it, as one fill and one stroke apiece.
 */
function paintedShapes(element, inherited = {}, found = [], context = "") {
  if (element == null || element === false) return found;
  if (Array.isArray(element)) {
    for (const child of element)
      paintedShapes(child, inherited, found, context);
    return found;
  }
  const { type, props } = element;
  if (typeof type === "function")
    return paintedShapes(type(props), inherited, found, context);
  if (typeof type !== "string")
    return paintedShapes(props.children, inherited, found, context);
  for (const key of Object.keys(props)) {
    const known =
      PAINT_PROPS.has(key) ||
      IGNORED_PROPS.has(key) ||
      (type === "svg"
        ? ["viewBox", "width", "height"].includes(key)
        : SHAPE_PROPS.has(key));
    if (!known)
      throw new Error(
        `${context}: the phone's icons ignore ${key} on <${type}>`,
      );
  }
  const translate = [...(inherited.translate ?? [0, 0])];
  if (props.transform) {
    const match = props.transform.match(
      /^translate\(\s*([-\d.]+)[ ,]+([-\d.]+)\s*\)$/,
    );
    if (!match) throw new Error(`${context}: only translate transforms draw`);
    translate[0] += number(match[1]);
    translate[1] += number(match[2]);
  }
  const own = Object.fromEntries(
    Object.entries(props).filter(
      ([key, value]) =>
        PAINT_PROPS.has(key) && key !== "opacity" && value !== undefined,
    ),
  );
  const paint = {
    ...inherited,
    ...own,
    translate,
    opacity: (inherited.opacity ?? 1) * number(props.opacity, 1),
  };
  if (type === "svg" || type === "g")
    return paintedShapes(props.children, paint, found, context);
  const d = outline(type, props);
  if (paint.fill === undefined)
    throw new Error(`${context}: a <${type}> has no fill; give it one`);
  if (paint.fill !== "none") found.push({ ...paint, d, style: "fill" });
  if (paint.stroke && paint.stroke !== "none") {
    if (paint.stroke !== "currentColor")
      throw new Error(
        `${context}: strokes are drawn only in the current colour`,
      );
    for (const end of [paint.strokeLinecap, paint.strokeLinejoin])
      if (end && end !== "round")
        throw new Error(
          `${context}: strokes are drawn only with round ends and joins`,
        );
    found.push({ ...paint, d, style: "stroke" });
  }
  return found;
}

function rootSvg(element) {
  while (typeof element.type === "function")
    element = element.type(element.props);
  if (element.type !== "svg") throw new Error("an icon must draw one <svg>");
  return element;
}

function phoneShapeLiteral(shape) {
  const style =
    shape.style === "fill"
      ? `.fill(${paint(shape.fill, null, {})})`
      : ".stroke";
  const width = number(shape.strokeWidth, 1.4);
  const dash = shape.strokeDasharray
    ? String(shape.strokeDasharray)
        .trim()
        .split(/[\s,]+/)
        .map((value) => number(value))
    : [];
  const extra =
    (shape.style === "stroke" && width !== 1.4 ? `, lineWidth: ${width}` : "") +
    (shape.style === "stroke" && dash.length
      ? `, dash: [${dash.join(", ")}]`
      : "");
  return (
    `IconShape(d: ${swiftString(shape.d)}, style: ${style}, opacity: ${fixed(shape.opacity)}, ` +
    `evenOdd: ${shape.fillRule === "evenodd"}, translate: (${shape.translate.join(", ")})${extra})`
  );
}

function phoneDef(key, element, context) {
  const svgElement = rootSvg(element);
  const body = paintedShapes(svgElement, {}, [], context).map(
    (shape) => `        ${phoneShapeLiteral(shape)},`,
  );
  return `    static let ${key} = IconDef(viewBox: ${viewBox(svgElement.props.viewBox)}, shapes: [\n${body.join("\n")}\n    ])`;
}

function solid(element) {
  const svgElement = rootSvg(element);
  const props = { ...svgElement.props };
  if (props.fill === "none" && props.stroke === "currentColor") {
    props.fill = "currentColor";
    props.stroke = "none";
  }
  return { ...svgElement, props };
}

function swiftName(name) {
  const bare = name.replace(/^Icon/, "");
  return bare[0].toLowerCase() + bare.slice(1);
}

const loader = await typescriptLoader();
const tsIcons = await loader.ssrLoadModule("/shared/ui/Icons.tsx");
const { DRAWN_ICONS } = await loader.ssrLoadModule(DRAWN_ICONS_PATH);
await loader.close();

const notchNames = Object.fromEntries(
  Object.entries(LINE_ICONS).map(([key, name]) => [name, key]),
);
const phoneLines = [];
const named = [];
const namedFilled = [];

for (const name of PHONE_ICONS) {
  if (MARK_NAMES[name]) {
    named.push(
      `        case ${swiftString(name)}: return mark(${swiftString(MARK_NAMES[name])})`,
    );
    continue;
  }
  const component = tsIcons[name];
  if (typeof component !== "function")
    throw new Error(`shared/ui/Icons.tsx has no ${name}`);
  const element = { type: component, props: { size: 16 } };
  const notchKey = notchNames[name];
  const filledByNotch = notchKey && FILLED_ICONS.has(notchKey);
  let key = notchKey;
  if (!notchKey || filledByNotch) {
    key = filledByNotch ? `${notchKey}Line` : swiftName(name);
    phoneLines.push(phoneDef(key, element, name));
  }
  named.push(`        case ${swiftString(name)}: return ${key}`);
  if (PHONE_FILLED.has(name)) {
    const filledKey = filledByNotch ? notchKey : `${key}Filled`;
    if (!filledByNotch)
      phoneLines.push(phoneDef(filledKey, solid(element), name));
    namedFilled.push(`        case ${swiftString(name)}: return ${filledKey}`);
  }
}

for (const [name, markup] of Object.entries(DRAWN_ICONS)) {
  const element = {
    type: "svg",
    props: {
      viewBox: "0 0 16 16",
      fill: "none",
      stroke: "currentColor",
      strokeWidth: 1.4,
      children: shapes(markup).map((shape) => ({
        type: "path",
        props: { d: shape.d },
      })),
    },
  };
  phoneLines.push(phoneDef(name, element, name));
  named.push(`        case ${swiftString(name)}: return ${name}`);
}

/** The marks the Mac puts before a link to these services, from its own icons. */
const SERVICE_FILES = {
  IconGithub: "github.svg",
  IconBitbucket: "bitbucket.svg",
  IconJira: "jira.svg",
  IconGitlab: "gitlab.svg",
  IconLinear: "linear.svg",
  IconVercel: "vercel.svg",
  IconSlack: "slack.svg",
  IconApple: "apple.svg",
  IconGoogleplay: "googleplay.svg",
};
const serviceDir = resolve(root, "app/assets/icons");

/** A shape's own `translate(..)scale(..)` as x and y scales and offsets. */
function scaling(transform) {
  let [sx, sy, tx, ty] = [1, 1, 0, 0];
  for (const [, kind, args] of (transform ?? "").matchAll(
    /(translate|scale)\(([^)]*)\)/g,
  )) {
    const [a, b] = args
      .trim()
      .split(/[\s,]+/)
      .map(Number);
    if (kind === "translate") {
      tx += sx * a;
      ty += sy * (b ?? 0);
    } else {
      sx *= a;
      sy *= b ?? a;
    }
  }
  return { sx, sy, tx, ty };
}

const PATH_ARGS = {
  m: 2,
  l: 2,
  h: 1,
  v: 1,
  c: 6,
  s: 4,
  q: 4,
  t: 2,
  a: 7,
  z: 0,
};

/** A path's data with a scaling baked into its numbers, since the phone's shapes only translate. */
function scaledPath(d, { sx, sy, tx, ty }) {
  const tokens = d.match(/[a-zA-Z]|[-+]?(?:\d*\.\d+|\d+\.?)(?:e[-+]?\d+)?/gi);
  const out = [];
  let command = "";
  let index = 0;
  for (const token of tokens) {
    if (/[a-z]/i.test(token)) {
      command = token;
      index = 0;
      out.push(token);
      continue;
    }
    const lower = command.toLowerCase();
    const relative = command === lower;
    const count = PATH_ARGS[lower];
    const at = index % count;
    index += 1;
    const value = Number(token);
    let axis;
    if (lower === "h") axis = "x";
    else if (lower === "v") axis = "y";
    else if (lower === "a")
      axis =
        at === 0
          ? "rx"
          : at === 1
            ? "ry"
            : at === 5
              ? "x"
              : at === 6
                ? "y"
                : null;
    else axis = at % 2 === 0 ? "x" : "y";
    const scaled =
      axis === "x"
        ? value * sx + (relative ? 0 : tx)
        : axis === "y"
          ? value * sy + (relative ? 0 : ty)
          : axis === "rx"
            ? value * sx
            : axis === "ry"
              ? value * sy
              : value;
    out.push(String(fixed(scaled)));
  }
  return out.join(" ");
}

for (const [name, file] of Object.entries(SERVICE_FILES)) {
  const svg = await readFile(resolve(serviceDir, file), "utf8");
  const box = svg.match(/viewBox="([^"]+)"/)?.[1];
  const [, , width, height] = box.split(/[\s,]+/).map(Number);
  const svgGradients = gradients(svg);
  const body = shapes(svg.replace(/<defs>[\s\S]*?<\/defs>/, "")).map(
    (shape) => {
      const scale = scaling(shape.transform);
      const id = shape.fill?.startsWith("url(#")
        ? shape.fill.slice(5, -1)
        : null;
      const gradient = id ? svgGradients[id] : null;
      const own = { ...svgGradients };
      if (gradient?.userSpace) {
        const place = ([x, y]) => [
          fixed((x * scale.sx + scale.tx) / width),
          fixed((y * scale.sy + scale.ty) / height),
        ];
        own[id] = {
          ...gradient,
          start: place(gradient.start),
          end: place(gradient.end),
        };
      }
      const baked = {
        ...shape,
        d: shape.transform ? scaledPath(shape.d, scale) : shape.d,
      };
      return `        ${shapeLiteral(baked, "fill", null, own)},`;
    },
  );
  const key = swiftName(name);
  phoneLines.push(
    `    static let ${key} = IconDef(viewBox: ${viewBox(box)}, shapes: [\n${body.join("\n")}\n    ])`,
  );
  named.push(`        case ${swiftString(name)}: return ${key}`);
}

const phoneSwift = `// Generated by scripts/generate-notch-icons.mjs from shared/ui/Icons.tsx and
// shared/ui/drawnIcons.ts, and app/assets/icons for the services' marks. Do not
// edit by hand.

extension Icons {
${phoneLines.join("\n")}

    /// The icon shared/ui/Icons.tsx exports as \`name\`, or one of the phone's own
    /// drawn icons by its key; \`filled\` asks for the solid version of a line icon.
    static func named(_ name: String, filled: Bool = false) -> IconDef? {
        if filled {
            switch name {
${namedFilled.map((line) => `    ${line}`).join("\n")}
            default: return nil
            }
        }
        switch name {
${named.join("\n")}
        default: return nil
        }
    }
}
`;

await emit(phoneOutputPath, phoneSwift);
