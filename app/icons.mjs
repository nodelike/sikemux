#!/usr/bin/env node
// Writes the icons the GPUI screens draw as SVG files, by rendering shared/ui/Icons.tsx,
// the set the phone and the notch draw too. The plugin marks, code-host section icons,
// outcome marks and phone QR code in assets/icons are kept as they are.

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const crate = import.meta.dirname;
const root = resolve(crate, "..");
const out = resolve(crate, "assets/icons");

const ICONS = "/shared/ui/Icons.tsx";

/** File name → the module, its export and the props it is drawn with. */
const DRAWN = {
  logo: [ICONS, "Logo"],
  plus: [ICONS, "IconPlus"],
  folder: [ICONS, "IconFolder"],
  "panel-left": [ICONS, "IconPanelLeft"],
  "panel-right": [ICONS, "IconPanelRight"],
  search: [ICONS, "IconSearch"],
  terminal: [ICONS, "IconTerminal"],
  globe: [ICONS, "IconGlobe"],
  stack: [ICONS, "IconStack"],
  agent: [ICONS, "IconAgent"],
  agents: [ICONS, "IconAgents"],
  file: [ICONS, "IconFile"],
  table: [ICONS, "IconTable"],
  editor: [ICONS, "IconEditor"],
  git: [ICONS, "IconGit"],
  board: [ICONS, "IconBoard"],
  check: [ICONS, "IconCheck"],
  grip: [ICONS, "IconGrip"],
  share: [ICONS, "IconShare"],
  more: [ICONS, "IconMore"],
  eye: [ICONS, "IconEye"],
  close: [ICONS, "IconClose"],
  chevron: [ICONS, "IconChevron"],
  "arrow-down": [ICONS, "IconArrowDown"],
  copy: [ICONS, "IconCopy"],
  text: [ICONS, "IconText"],
  discard: [ICONS, "IconDiscard"],
  inbox: [ICONS, "IconInbox"],
  "log-in": [ICONS, "IconLogIn"],
  "log-out": [ICONS, "IconLogOut"],
  settings: [ICONS, "IconSettings"],
  command: [ICONS, "IconCommand"],
  contrast: [ICONS, "IconContrast"],
  focus: [ICONS, "IconFocus"],
  pencil: [ICONS, "IconPencil"],
  server: [ICONS, "IconServer"],
  window: [ICONS, "IconWindow"],
  claude: [ICONS, "IconClaude"],
  hermes: [ICONS, "IconHermes"],
  pi: [ICONS, "IconPi"],
  opencode: [ICONS, "IconOpenCode"],
  grok: [ICONS, "IconGrok"],
  run: [ICONS, "IconRun"],
  stop: [ICONS, "IconStop"],
  refresh: [ICONS, "IconRefresh"],
  laptop: [ICONS, "IconLaptop"],
  plug: [ICONS, "IconPlug"],
  "pull-request": [ICONS, "IconPullRequest"],
  shield: [ICONS, "IconShield"],
  "shield-bolt": [ICONS, "IconShieldBolt"],
  "arrow-up": [ICONS, "IconArrowUp"],
  mic: [ICONS, "IconMic"],
  "file-plus": [ICONS, "IconFilePlus"],
  "folder-plus": [ICONS, "IconFolderPlus"],
  "collapse-all": [ICONS, "IconCollapseAll"],
  aws: [ICONS, "IconAws"],
  rundeck: [ICONS, "IconRundeck"],
  activity: [ICONS, "IconActivity"],
  book: [ICONS, "IconBook"],
  user: [ICONS, "IconUser"],
  building: [ICONS, "IconBuilding"],
  home: [ICONS, "IconHome"],
  briefcase: [ICONS, "IconBriefcase"],
  flask: [ICONS, "IconFlask"],
  star: [ICONS, "IconStar"],
  heart: [ICONS, "IconHeart"],
  bolt: [ICONS, "IconBolt"],
  leaf: [ICONS, "IconLeaf"],
  cube: [ICONS, "IconCube"],
  music: [ICONS, "IconMusic"],
  camera: [ICONS, "IconCamera"],
  click: [ICONS, "IconClick"],
  clock: [ICONS, "IconClock"],
  code: [ICONS, "IconCode"],
  dashboard: [ICONS, "IconDashboard"],
  download: [ICONS, "IconDownload"],
  highlighter: [ICONS, "IconHighlighter"],
  "home-button": [ICONS, "IconHomeButton"],
  hourglass: [ICONS, "IconHourglass"],
  gauge: [ICONS, "IconGauge"],
  issue: [ICONS, "IconIssue"],
  keyboard: [ICONS, "IconKeyboard"],
  link: [ICONS, "IconLink"],
  "log-lines": [ICONS, "IconLogLines"],
  message: [ICONS, "IconMessage"],
  mouse: [ICONS, "IconMouse"],
  move: [ICONS, "IconMove"],
  network: [ICONS, "IconNetwork"],
  panel: [ICONS, "IconPanel"],
  phone: [ICONS, "IconPhone"],
  pinch: [ICONS, "IconPinch"],
  pointer: [ICONS, "IconPointer"],
  record: [ICONS, "IconRecord"],
  resize: [ICONS, "IconResize"],
  rocket: [ICONS, "IconRocket"],
  rotate: [ICONS, "IconRotate"],
  pin: [ICONS, "IconPin"],
  power: [ICONS, "IconPower"],
  "square-plus": [ICONS, "IconSquarePlus"],
  status: [ICONS, "IconStatus"],
  steps: [ICONS, "IconSteps"],
  swipe: [ICONS, "IconSwipe"],
  tabs: [ICONS, "IconTabs"],
  tap: [ICONS, "IconTap"],
  "touch-path": [ICONS, "IconTouchPath"],
  trash: [ICONS, "IconTrash"],
  unlink: [ICONS, "IconUnlink"],
  upload: [ICONS, "IconUpload"],
  warning: [ICONS, "IconWarning"],
  "info-circle": [ICONS, "IconInfo"],
  waterfall: [ICONS, "IconWaterfall"],
  workflow: [ICONS, "IconWorkflow"],
  wrench: [ICONS, "IconWrench"],
  timer: [ICONS, "IconTimer"],
  // The marks of a toast, named as GPUI Kit's notifications ask for them.
  info: [ICONS, "IconInfoMark"],
  "circle-check": [ICONS, "IconCheck"],
  "circle-x": [ICONS, "IconExclamation"],
  "triangle-alert": [ICONS, "IconWarning"],
};
/** Marks the app draws as pictures of their own, copied as they are. */
const PICTURES = {
  codex: "shared/assets/agent-logos/codex.svg",
  omp: "shared/assets/agent-logos/omp.svg",
  muse: "shared/assets/agent-logos/muse.svg",
  bruno: "shared/assets/bruno.svg",
};
/** GPUI's SVG renderer has no oklch(), which WebKit draws, so a picture's oklch colours are
 * written as the sRGB they come to. */
const OKLCH = /oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\)/g;
function oklchToHex(l, c, h) {
  const radians = (h * Math.PI) / 180;
  const [a, b] = [c * Math.cos(radians), c * Math.sin(radians)];
  const cube = (value) => value ** 3;
  const [lms1, lms2, lms3] = [
    cube(l + 0.3963377774 * a + 0.2158037573 * b),
    cube(l - 0.1055613458 * a - 0.0638541728 * b),
    cube(l - 0.0894841775 * a - 1.291485548 * b),
  ];
  const linear = [
    4.0767416621 * lms1 - 3.3077115913 * lms2 + 0.2309699292 * lms3,
    -1.2684380046 * lms1 + 2.6097574011 * lms2 - 0.3413193965 * lms3,
    -0.0041960863 * lms1 - 0.7034186147 * lms2 + 1.707614701 * lms3,
  ];
  const encoded = linear.map((value) => {
    const clamped = Math.min(1, Math.max(0, value));
    const srgb = clamped <= 0.0031308 ? 12.92 * clamped : 1.055 * clamped ** (1 / 2.4) - 0.055;
    return Math.round(srgb * 255).toString(16).padStart(2, "0");
  });
  return `#${encoded.join("")}`;
}
/** Marks with colours of their own beside a part in the text's colour, which a
 * picture cannot follow, so that part takes the rail's body ink. */
const MIXED = { aws: "#acabb4" };

const server = await createServer({
  root,
  logLevel: "error",
  server: { middlewareMode: true, hmr: false },
  appType: "custom",
  optimizeDeps: { noDiscovery: true, include: [] },
});
try {
  await mkdir(out, { recursive: true });
  for (const [file, [module, name, props = {}]] of Object.entries(DRAWN)) {
    const component = (await server.ssrLoadModule(module))[name];
    const markup = renderToStaticMarkup(createElement(component, { size: 16, ...props }))
      .replace(/^<svg[^>]*>/, (root) => root.replace(/ (width|height|class|aria-hidden)="[^"]*"/g, ""))
      .replace("<svg", '<svg xmlns="http://www.w3.org/2000/svg"')
      .replaceAll("currentColor", MIXED[file] ?? "currentColor");
    await writeFile(resolve(out, `${file}.svg`), `${markup}\n`);
  }
  for (const [file, path] of Object.entries(PICTURES))
    await writeFile(resolve(out, `${file}.svg`), (await readFile(resolve(root, path), "utf8")).replace(OKLCH, (_, l, c, h) => oklchToHex(+l, +c, +h)));
  const { BATTERY_BOLT } = await server.ssrLoadModule(ICONS);
  const bolt = (extra) =>
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="7 -3 12 19" fill="currentColor"><path d="${BATTERY_BOLT}"${extra}></path></svg>\n`;
  const chevron = await readFile(resolve(out, "chevron.svg"), "utf8");
  await writeFile(
    resolve(out, "chevron-down.svg"),
    chevron.replace(/^(<svg[^>]*>)(.*)(<\/svg>)/s, '$1<g transform="rotate(90 8 8)">$2</g>$3'),
  );
  await writeFile(resolve(out, "battery-bolt.svg"), bolt(""));
  await writeFile(resolve(out, "battery-bolt-gap.svg"), bolt(' stroke="currentColor" stroke-width="2.4" stroke-linejoin="round"'));
} finally {
  await server.close();
}
