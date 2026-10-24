#!/usr/bin/env node
// Writes the agent faces from the app's own art in src/buddies: the catalogue
// the core resolves each agent's face from, and every drawing of each shape for
// the GPUI app. `--check` fails when either is out of date instead of writing.
// Each shape's crop is measured in Chrome, as the app measures it in its page,
// and kept beside a digest of the drawing it was measured on, so the check needs
// no browser and only a changed drawing is measured again.

import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { gunzipSync, gzipSync } from "node:zlib";
import { chromium } from "playwright-core";
import { createServer } from "vite";

const root = resolve(import.meta.dirname, "..");
const manifestPath = resolve(
  root,
  "src-tauri/crates/sikemux-core/src/assets/faces.json",
);
const drawingsDir = resolve(root, "src-tauri/crates/sikemux-gpui/assets/faces");
const check = process.argv.includes("--check");
const STATES = ["idle", "working", "waiting", "done", "error"];
// The art repeats an attribute on some elements. WebKit, which draws it on the Mac,
// keeps the first; an SVG parser refuses the whole drawing. Keep the first here too.
const firstOfEachAttribute = (markup) =>
  markup.replace(
    /<([a-zA-Z][\w:-]*)((?:\s+[\w:.-]+="[^"]*")*)(\s*\/?)>/g,
    (_, name, attributes, end) => {
      const seen = new Set();
      const kept = attributes.replace(
        /\s+([\w:.-]+)="[^"]*"/g,
        (attribute, key) => (seen.has(key) ? "" : (seen.add(key), attribute)),
      );
      return `<${name}${kept}${end}>`;
    },
  );

const server = await createServer({
  root,
  logLevel: "error",
  server: { middlewareMode: true, hmr: false },
  appType: "custom",
  optimizeDeps: { noDiscovery: true, include: [] },
});
const wanted = new Map();
try {
  const { BUDDY_PACKS } = await server.ssrLoadModule(
    "/src/buddies/catalogue.ts",
  );
  const { buddyMarkup, boxAround, measuredDrawing, FULL_BOX } =
    await server.ssrLoadModule("/src/buddies/render.ts");
  const kept = JSON.parse(
    await readFile(manifestPath, "utf8").catch(() => "{}"),
  );
  const keptBoxes = new Map(
    (kept.packs ?? []).flatMap((pack) =>
      pack.shapes.map((shape) => [`${pack.id}/${shape.id}`, shape]),
    ),
  );
  const digest = (drawing) =>
    createHash("sha256").update(drawing).digest("hex").slice(0, 16);
  const toMeasure = [];
  const packs = BUDDY_PACKS.map((pack) => ({
    id: pack.id,
    name: pack.name,
    shapes: pack.shapes.map((shape) => {
      const drawing = measuredDrawing(pack, shape);
      const measured = digest(drawing);
      const known = keptBoxes.get(`${pack.id}/${shape.id}`);
      if (known?.measured === measured)
        return { id: shape.id, name: shape.name, box: known.box, measured };
      const entry = { id: shape.id, name: shape.name, box: null, measured };
      toMeasure.push({ entry, drawing });
      return entry;
    }),
    colors: pack.colors.length,
  }));
  if (toMeasure.length && !check) {
    const browser = await chromium.launch({ channel: "chrome" });
    try {
      const page = await browser.newPage();
      const bounds = await page.evaluate(
        (drawings) =>
          drawings.map((drawing) => {
            const svg = document.createElementNS(
              "http://www.w3.org/2000/svg",
              "svg",
            );
            svg.setAttribute("style", "width:24px;height:24px");
            svg.innerHTML = drawing;
            document.body.appendChild(svg);
            const { x, y, width, height } = svg.firstElementChild.getBBox();
            svg.remove();
            return { x, y, width, height };
          }),
        toMeasure.map(({ drawing }) => drawing),
      );
      toMeasure.forEach(({ entry }, index) => {
        const b = bounds[index];
        entry.box = b.width > 0 && b.height > 0 ? boxAround(b) : FULL_BOX;
      });
    } finally {
      await browser.close();
    }
  }
  wanted.set(
    manifestPath,
    Buffer.from(`${JSON.stringify({ packs }, null, 2)}\n`),
  );
  for (const pack of BUDDY_PACKS) {
    for (const shape of pack.shapes) {
      const drawings = pack.colors.map((color) =>
        Object.fromEntries(
          STATES.map((state) => [
            state,
            firstOfEachAttribute(
              buddyMarkup(pack, shape, color, state, "face", FULL_BOX),
            ).replace("<svg ", '<svg xmlns="http://www.w3.org/2000/svg" '),
          ]),
        ),
      );
      wanted.set(
        resolve(drawingsDir, `${pack.id}.${shape.id}.json.gz`),
        gzipSync(JSON.stringify(drawings), { level: 9 }),
      );
    }
  }
} finally {
  await server.close();
}

const present = (await readdir(drawingsDir).catch(() => [])).map((file) =>
  resolve(drawingsDir, file),
);
const extra = present.filter((path) => !wanted.has(path));
// Compressed files are compared by what they hold, since zlib versions differ in the bytes they write.
const unpacked = (path, bytes) =>
  path.endsWith(".gz") ? gunzipSync(bytes) : bytes;
const stale = [];
for (const [path, bytes] of wanted) {
  const current = await readFile(path).catch(() => null);
  if (!current || !unpacked(path, current).equals(unpacked(path, bytes)))
    stale.push(path);
}
if (check) {
  const wrong = [...stale, ...extra].map((path) => path.slice(root.length + 1));
  if (wrong.length) {
    console.error(
      `Agent faces are out of date; run node scripts/generate-faces.mjs:\n  ${wrong.join("\n  ")}`,
    );
    process.exit(1);
  }
} else {
  await mkdir(drawingsDir, { recursive: true });
  await mkdir(resolve(manifestPath, ".."), { recursive: true });
  for (const path of extra) await rm(path);
  for (const path of stale) await writeFile(path, wanted.get(path));
}
