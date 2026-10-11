import {
  copyFile,
  mkdir,
  readdir,
  readFile,
  writeFile,
} from "node:fs/promises";
import { createServer as createNetServer } from "node:net";
import { resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";
import { createServer } from "vite";
import { PAGES } from "./pages.mjs";
import { README_SCREENSHOTS, SCENES } from "./scenes.mjs";

const root = resolve(import.meta.dirname, "..");
const { values: options, positionals: only } = parseArgs({
  allowPositionals: true,
  options: {
    out: { type: "string", default: resolve(root, "showcase/out") },
    width: { type: "string", default: "1728" },
    height: { type: "string", default: "1080" },
    publish: { type: "boolean", default: false },
    "refresh-pages": { type: "boolean", default: false },
    site: { type: "string" },
  },
});

const MAC_USER_AGENT =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const FIXED_TIME = new Date("2026-09-26T09:41:00");
const viewport = {
  width: Number(options.width),
  height: Number(options.height),
};

/** `pnpm showcase:serve` keeps 1471, so a capture takes whichever port is free. */
const freePort = () =>
  new Promise((settle, fail) => {
    const probe = createNetServer().listen(0, "localhost", () => {
      const { port } = probe.address();
      probe.close(() => settle(port));
    });
    probe.on("error", fail);
  });

const server = await createServer({
  configFile: resolve(root, "showcase/vite.config.ts"),
  logLevel: "warn",
  server: { port: await freePort(), strictPort: true },
});
await server.listen();
const origin = `http://localhost:${server.config.server.port}`;

// The chat's markdown comes from the app's Rust parser, which may need building first;
// without this the first scene is taken before its prose arrives.
await fetch(`${origin}/__showcase/markdown`, {
  method: "POST",
  body: JSON.stringify({ requests: [{ text: "ready" }] }),
});

const browser = await chromium.launch({ channel: "chrome" });
const context = await browser.newContext({
  viewport,
  deviceScaleFactor: 2,
  userAgent: MAC_USER_AGENT,
  colorScheme: "dark",
});
await mkdir(options.out, { recursive: true });

if (options["refresh-pages"]) {
  const pages = await browser.newContext({
    deviceScaleFactor: 2,
    colorScheme: "dark",
    userAgent: MAC_USER_AGENT,
  });
  for (const { name, url, width, height } of PAGES) {
    const page = await pages.newPage();
    await page.setViewportSize({ width, height });
    await page.goto(url, { waitUntil: "networkidle" });
    await page.screenshot({
      path: resolve(root, "showcase/pages", `${name}.png`),
    });
    await page.close();
    console.log(`snapshotted ${url}`);
  }
  await pages.close();
}

const scenes = SCENES.filter(
  (scene) => only.length === 0 || only.includes(scene.name),
);
const problems = [];

for (const scene of scenes) {
  const page = await context.newPage();
  page.on("pageerror", (error) =>
    problems.push(`${scene.name}: ${error.message.split("\n")[0]}`),
  );
  await page.clock.setFixedTime(FIXED_TIME);
  await page.goto(`${origin}/showcase/`);
  await page.waitForFunction(
    () => window.showcase && document.querySelector(".shell"),
  );
  await page.waitForTimeout(600);
  try {
    await scene.setup(page);
  } catch (error) {
    problems.push(
      `${scene.name}: setup failed, ${error.message.split("\n")[0]}`,
    );
    await page.close();
    continue;
  }
  const nextFrame = () =>
    page.evaluate(
      () =>
        new Promise((done) =>
          requestAnimationFrame(() => requestAnimationFrame(done)),
        ),
    );
  let now = FIXED_TIME.getTime();
  for (;;) {
    const hold = await page.evaluate(() => window.showcase.backend.stepLive());
    if (hold < 0) break;
    await nextFrame();
    now += hold;
    await page.clock.setFixedTime(now);
  }
  await page.waitForTimeout(scene.settle ?? 900);

  const full = resolve(options.out, `${scene.name}.png`);
  await page.screenshot({ path: full });
  await frame(full, resolve(options.out, `${scene.name}-framed.png`));
  for (const [name, crop] of Object.entries(scene.crops ?? {})) {
    const { selector, region } =
      typeof crop === "string" ? { selector: crop } : crop;
    const target = page.locator(selector).first();
    if ((await target.count()) === 0) {
      problems.push(
        `${scene.name}: crop "${name}" matched nothing (${selector})`,
      );
      continue;
    }
    const path = resolve(options.out, `${scene.name}-${name}.png`);
    if (!region) {
      await target.screenshot({ path });
      continue;
    }
    // A region is a fraction of the element's box, so a crop keeps its framing when the layout shifts.
    const box = await target.boundingBox();
    await page.screenshot({
      path,
      clip: {
        x: box.x + box.width * region.left,
        y: box.y + box.height * region.top,
        width: box.width * region.width,
        height: box.height * region.height,
      },
    });
  }
  const unhandled = await page.evaluate(() => [
    ...window.showcase.backend.unhandled.keys(),
  ]);
  console.log(
    `✓ ${scene.name}${unhandled.length ? `  (unfaked: ${unhandled.join(", ")})` : ""}`,
  );
  await page.close();
}

async function frame(source, target) {
  const page = await context.newPage();
  const image = `data:image/png;base64,${(await readFile(source)).toString("base64")}`;
  const pad = 72;
  await page.setViewportSize({
    width: viewport.width + pad * 2,
    height: viewport.height + pad * 2,
  });
  await page.setContent(`<!doctype html><html><body style="margin:0;display:grid;place-items:center;height:100vh;background:radial-gradient(120% 90% at 20% 0%, #2a2140 0%, #120f1a 55%, #0b0a10 100%)">
        <img src="${image}" style="width:${viewport.width}px;height:${viewport.height}px;border-radius:12px;box-shadow:0 0 0 1px rgba(255,255,255,.09),0 1px 0 rgba(255,255,255,.06) inset,0 30px 80px rgba(0,0,0,.55),0 8px 24px rgba(0,0,0,.35)">
    </body></html>`);
  await page.screenshot({ path: target });
  await page.close();
}

await browser.close();
await server.close();

if (options.publish) {
  for (const [capture, screenshot] of Object.entries(README_SCREENSHOTS)) {
    await copyFile(
      resolve(options.out, `${capture}.png`),
      resolve(root, "public/screenshots", screenshot),
    );
  }
  console.log(
    `published ${Object.keys(README_SCREENSHOTS).length} README screenshots`,
  );
}
if (options.site) {
  await mkdir(options.site, { recursive: true });
  const captures = (await readdir(options.out)).filter((name) =>
    name.endsWith(".png"),
  );
  for (const name of captures)
    await copyFile(resolve(options.out, name), resolve(options.site, name));
  console.log(`copied ${captures.length} captures to ${options.site}`);
}
if (problems.length) {
  await writeFile(resolve(options.out, "problems.txt"), problems.join("\n"));
  console.log(`\n${problems.length} problem(s):\n${problems.join("\n")}`);
  process.exitCode = 1;
}
