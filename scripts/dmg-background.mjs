import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright-core";

const ROOT = new URL("..", import.meta.url).pathname;
const OUT = join(ROOT, "src-tauri/dmg/background.tiff");
const config = JSON.parse(
  readFileSync(join(ROOT, "src-tauri/tauri.macos.conf.json"), "utf8"),
).bundle.macOS.dmg;
const { width, height } = config.windowSize;
const app = config.appPosition;
const folder = config.applicationFolderPosition;

// Finder counts the title bar in the window height and covers that much of the picture.
const visibleHeight = height - 32;
const arrowStart = app.x + 84;
const arrowEnd = folder.x - 84;

const page = `<!doctype html>
<html><head><style>
  html, body { margin: 0; width: ${width}px; height: ${height}px; overflow: hidden; }
  body {
    position: relative;
    background:
      radial-gradient(ellipse 60% 55% at 50% ${app.y}px, rgba(162, 119, 255, 0.16), transparent 70%),
      radial-gradient(ellipse 120% 90% at 50% 0%, #ffffff, transparent 70%),
      linear-gradient(#f6f4fb, #e6e1f2);
    font-family: -apple-system, "SF Pro Text", "Helvetica Neue", sans-serif;
  }
  svg { position: absolute; left: 0; top: 0; }
  .caption {
    position: absolute; left: 0; right: 0; top: ${visibleHeight - 64}px; text-align: center;
    font-size: 13px; letter-spacing: 0.01em; color: rgba(28, 22, 48, 0.5);
  }
  .caption b { color: rgba(28, 22, 48, 0.82); font-weight: 500; }
</style></head><body>
  <svg width="${width}" height="${height}">
    <defs><linearGradient id="fade" gradientUnits="userSpaceOnUse" x1="${arrowStart}" x2="${arrowEnd}">
      <stop offset="0" stop-color="#7c4dff" stop-opacity="0"/>
      <stop offset="1" stop-color="#7c4dff" stop-opacity="0.9"/>
    </linearGradient></defs>
    <path d="M${arrowStart} ${app.y} H${arrowEnd}" stroke="url(#fade)" stroke-width="1.5" stroke-dasharray="2 5" stroke-linecap="round" fill="none"/>
    <path d="M${arrowEnd - 6} ${app.y - 6} L${arrowEnd} ${app.y} L${arrowEnd - 6} ${app.y + 6}" stroke="#7c4dff" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
  </svg>
  <div class="caption">Drag <b>Sikemux</b> into <b>Applications</b> to install</div>
</body></html>`;

const scratch = mkdtempSync(join(tmpdir(), "sikemux-dmg-"));
const browser = await chromium.launch({ channel: "chrome" });
const shots = [];
for (const scale of [1, 2]) {
  const tab = await browser.newPage({
    viewport: { width, height },
    deviceScaleFactor: scale,
  });
  await tab.setContent(page);
  const path = join(
    scratch,
    scale === 1 ? "background.png" : "background@2x.png",
  );
  await tab.screenshot({ path });
  shots.push(path);
  await tab.close();
}
await browser.close();

execFileSync("tiffutil", ["-cathidpicheck", ...shots, "-out", OUT]);
rmSync(scratch, { recursive: true, force: true });
console.log(`DMG background written to ${OUT}`);
