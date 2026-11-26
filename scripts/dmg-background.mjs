import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright-core";

const ROOT = new URL("..", import.meta.url).pathname;
const OUT = join(ROOT, "packaging/dmg/background.tiff");
const [width, height] = [660, 400];
const app = { x: 168, y: 194 };
const folder = { x: 492, y: 194 };

const dataUri = (rel, mime) =>
  `data:${mime};base64,${readFileSync(join(ROOT, rel)).toString("base64")}`;

const font400 = dataUri(
  "helpers/notch/Fonts/Figtree_400Regular.ttf",
  "font/ttf",
);
const font500 = dataUri(
  "helpers/notch/Fonts/Figtree_500Medium.ttf",
  "font/ttf",
);
const lockupWhite = dataUri("brand/wordmark/lockup-white.svg", "image/svg+xml");

const page = `<!doctype html>
<html><head><style>
  @font-face { font-family: Figtree; font-weight: 400; src: url("${font400}"); }
  @font-face { font-family: Figtree; font-weight: 500; src: url("${font500}"); }
  html, body { margin: 0; width: ${width}px; height: ${height}px; overflow: hidden; background: #08080a; }
  canvas { display: block; width: ${width}px; height: ${height}px; }
</style></head><body>
<canvas id="c"></canvas>
<script>
const [W, H] = [${width}, ${height}];
const [ax, ay] = [${app.x}, ${app.y}];
const [fx, fy] = [${folder.x}, ${folder.y}];
const LOCKUP = "${lockupWhite}";

const clamp = (x, a = 0, b = 1) => Math.min(b, Math.max(a, x));
const ss = (a, b, x) => { const t = clamp((x - a) / (b - a)); return t * t * (3 - 2 * t); };
const mix = (a, b, t) => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
const fract = (x) => x - Math.floor(x);
function hash(x, y) {
  x = fract(x * 123.34); y = fract(y * 456.21);
  const d = x * (x + 45.32) + y * (y + 45.32);
  return fract((x + d) * (y + d));
}
function noise(x, y) {
  const ix = Math.floor(x), iy = Math.floor(y);
  let fx = x - ix, fy = y - iy;
  fx = fx * fx * (3 - 2 * fx); fy = fy * fy * (3 - 2 * fy);
  const a = hash(ix, iy), b = hash(ix + 1, iy), c = hash(ix, iy + 1), d = hash(ix + 1, iy + 1);
  return (a + (b - a) * fx) + ((c + (d - c) * fx) - (a + (b - a) * fx)) * fy;
}
function rolls(seed) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let v = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    v = (v + Math.imul(v ^ (v >>> 7), 61 | v)) ^ v;
    return ((v ^ (v >>> 14)) >>> 0) / 4294967296;
  };
}
const gauss = (roll) => (roll() + roll() + roll() - 1.5) * 1.15;
const ACCENT = [162 / 255, 119 / 255, 1];
const rgb = (c, a = 1) => \`rgba(\${Math.round(c[0] * 255)},\${Math.round(c[1] * 255)},\${Math.round(c[2] * 255)},\${a})\`;
const rectDist = (x, y, [a, b, d, e]) => Math.hypot(Math.max(a - x, 0, x - d), Math.max(b - y, 0, y - e));

function lamp(ctx, x, y, rx, ry, color, strength) {
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(1, ry / rx);
  const g = ctx.createRadialGradient(0, 0, 0, 0, 0, rx);
  for (let i = 0; i <= 10; i++) {
    const r = (i / 10) * 2.1;
    g.addColorStop(i / 10, rgb(color, strength * Math.exp(-r * r)));
  }
  ctx.fillStyle = g;
  ctx.fillRect(-rx, -rx, rx * 2, rx * 2);
  ctx.restore();
}

window.render = async (scale) => {
  await document.fonts.load("500 12.5px Figtree");
  await document.fonts.load("400 12.5px Figtree");
  const canvas = document.getElementById("c");
  canvas.width = W * scale;
  canvas.height = H * scale;
  const ctx = canvas.getContext("2d");
  ctx.scale(scale, scale);

  const bg = ctx.createLinearGradient(0, 0, 0, H);
  bg.addColorStop(0, "#101013");
  bg.addColorStop(0.5, "#0a0a0c");
  bg.addColorStop(1, "#070709");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, W, H);

  lamp(ctx, W * 0.5, 16, 440, 260, [1, 1, 1], 0.055);
  lamp(ctx, ax, ay, 130, 130, [1, 1, 1], 0.04);
  lamp(ctx, fx, fy, 135, 135, [1, 1, 1], 0.036);

  const arrowBox = [ax + 72, ay - 15, fx - 72, ay + 15];
  const appBox = [ax - 56, ay - 56, ax + 56, ay + 82];
  const foldBox = [fx - 58, fy - 58, fx + 58, fy + 82];
  const capBox = [205, 348, 455, 378];
  const headBox = [265, 22, 395, 58];

  const mask = (x, y) => {
    const edge = ss(0, 82, x) * ss(0, 82, W - x) * ss(10, 56, y) * ss(10, 52, H - y);
    const quiet =
      ss(0, 34, rectDist(x, y, appBox)) *
      ss(0, 34, rectDist(x, y, foldBox)) *
      ss(0, 28, rectDist(x, y, arrowBox)) *
      ss(0, 20, rectDist(x, y, capBox)) *
      ss(0, 20, rectDist(x, y, headBox));
    return edge * quiet;
  };

  const roll = rolls(23);
  const strands = 20, perStrand = 3400, amplitude = 96, phaseShift = 0.4;
  for (let s = 0; s < strands; s++) {
    const sn = s / (strands - 1);
    const depth = (sn - 0.5) * 2;
    const phase = sn * Math.PI * 1.45 + phaseShift;
    const freq1 = 1.65 + 0.35 * Math.sin(sn * 3.1);
    const freq2 = 3.2 - 0.4 * Math.cos(sn * 2.4);
    const spreadY = 3.2 + 10.5 * (depth * depth);

    for (let i = 0; i < perStrand; i++) {
      const t = (i + roll()) / perStrand;
      const x = -12 + t * (W + 24);
      const nx = (t - 0.5) * 2;
      const env = 0.55 + 0.52 * Math.sin(t * Math.PI) * (0.7 + 0.3 * Math.cos(nx * 2.4));
      const yWave =
        Math.sin(nx * Math.PI * freq1 + phase) * amplitude * 0.62 * env +
        Math.cos(nx * Math.PI * freq2 - phase * 0.7) * amplitude * 0.34 * env +
        depth * 42 * Math.sin(nx * Math.PI * 1.1 + 0.4);

      const turb = (noise(x / 95 + sn * 3.1, (ay + yWave) / 95) - 0.5) * 18;
      const g1 = gauss(roll);
      const y = ay + yWave + turb + g1 * spreadY;
      const px = x + gauss(roll) * 2.2;
      if (px < -4 || px > W + 4 || y < -4 || y > H + 4) continue;

      const m = mask(px, y);
      if (m < 0.015) continue;

      const ridge = Math.exp(-((g1 * 1.35) ** 2));
      const fold = Math.exp(-((depth * 1.2) ** 2));
      const cloud = noise(px / 110 + 4.2, y / 110 - 2.1);
      const density = m * (0.45 + 0.55 * ss(0.14, 0.86, cloud));
      const u1 = roll(), u2 = roll();

      const centerFocus = Math.exp(-(((px - W * 0.5) / 185) ** 2 + ((y - ay) / 115) ** 2));
      let col = mix([0.42, 0.43, 0.47], [0.95, 0.96, 0.99], clamp(ridge * 0.72 + fold * 0.35));
      if (ridge > 0.72 && u1 > 0.72) col = [1, 1, 1];
      if (ridge > 0.55 && centerFocus > 0.32 && u2 > 0.74 && u2 < 0.91) {
        col = mix(col, ACCENT, 0.88);
      }

      if (u1 > 0.994 && ridge > 0.55) {
        const radius = 2 + u2 * 3.4;
        const rr = radius + 4.5;
        const a = 0.13 * density;
        const bc = mix([1, 1, 1], ACCENT, centerFocus * 0.35);
        const g = ctx.createRadialGradient(px, y, 0, px, y, rr);
        g.addColorStop(0, rgb(bc, a));
        g.addColorStop(0.5, rgb(bc, a * 0.42));
        g.addColorStop(1, rgb(bc, 0));
        ctx.fillStyle = g;
        ctx.beginPath(); ctx.arc(px, y, rr, 0, 6.2832); ctx.fill();
      } else {
        const alpha = clamp((0.06 + 0.56 * ridge * (0.45 + 0.55 * fold)) * density);
        if (alpha < 0.014) continue;
        const radius = 0.38 + 0.68 * (u1 ** 3) + 0.28 * ridge * fold;
        ctx.fillStyle = rgb(col, alpha);
        ctx.beginPath(); ctx.arc(px, y, radius, 0, 6.2832); ctx.fill();
      }
    }
  }

  // Drop well around Applications
  const size = 120, half = size / 2, r = 24;
  ctx.save();
  ctx.fillStyle = "rgba(255,255,255,0.028)";
  ctx.beginPath(); ctx.roundRect(fx - half, fy - half, size, size, r); ctx.fill();
  ctx.strokeStyle = "rgba(255,255,255,0.12)"; ctx.lineWidth = 1; ctx.stroke();
  ctx.strokeStyle = "rgba(255,255,255,0.22)"; ctx.lineWidth = 1;
  ctx.beginPath(); ctx.moveTo(fx - half + r, fy - half + 0.5); ctx.lineTo(fx + half - r, fy - half + 0.5); ctx.stroke();
  ctx.restore();

  // Finder renders custom-background icon labels in black regardless of appearance,
  // so neutral silver pills keep Sikemux and Applications crisp on obsidian.
  for (const [lx, pw] of [[ax, 86], [fx, 108]]) {
    const ph = 22, top = 268.5 - ph / 2;
    ctx.save();
    ctx.fillStyle = "rgba(232,232,236,0.92)";
    ctx.beginPath(); ctx.roundRect(lx - pw / 2, top, pw, ph, ph / 2); ctx.fill();
    ctx.strokeStyle = "rgba(255,255,255,0.5)"; ctx.lineWidth = 0.75;
    ctx.stroke();
    ctx.restore();
  }

  // Prominent sculpted white arrow
  const x0 = ax + 82, x1 = fx - 82, head = 12;
  ctx.save();
  ctx.strokeStyle = "rgba(9,9,11,0.78)"; ctx.lineWidth = 21.5; ctx.lineCap = "round";
  ctx.beginPath(); ctx.moveTo(x0, ay); ctx.lineTo(x1, ay); ctx.stroke();
  ctx.strokeStyle = "#ffffff"; ctx.lineWidth = 3.5; ctx.lineCap = "round"; ctx.lineJoin = "round";
  ctx.beginPath(); ctx.moveTo(x0, ay); ctx.lineTo(x1 - 2, ay); ctx.stroke();
  ctx.beginPath(); ctx.moveTo(x1 - head, ay - head * 0.86); ctx.lineTo(x1, ay); ctx.lineTo(x1 - head, ay + head * 0.86); ctx.stroke();
  ctx.restore();

  const lockup = await new Promise((done, fail) => {
    const img = new Image();
    img.onload = () => done(img);
    img.onerror = fail;
    img.src = LOCKUP;
  });
  const lh = 15, lw = lh * (758.27 / 157.14);
  ctx.globalAlpha = 0.9;
  ctx.drawImage(lockup, W / 2 - lw / 2, 32, lw, lh);
  ctx.globalAlpha = 1;

  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  const dim = "rgba(255,255,255,.42)", strong = "rgba(255,255,255,.90)";
  const parts = [["Drag ", dim], ["Sikemux", strong], [" to ", dim], ["Applications", strong]];
  const fontOf = (color) => (color === strong ? "500 12.5px Figtree" : "400 12.5px Figtree");
  let total = 0;
  for (const [text, color] of parts) { ctx.font = fontOf(color); total += ctx.measureText(text).width; }
  let cx = W / 2 - total / 2;
  for (const [text, color] of parts) {
    ctx.font = fontOf(color);
    ctx.fillStyle = color;
    ctx.fillText(text, cx, 366);
    cx += ctx.measureText(text).width;
  }
};
</script></body></html>`;

const scratch = mkdtempSync(join(tmpdir(), "sikemux-dmg-"));
try {
  const browser = await chromium.launch({ channel: "chrome" });
  const tifs = [];
  for (const scale of [1, 2]) {
    const tab = await browser.newPage({
      viewport: { width, height },
      deviceScaleFactor: scale,
    });
    await tab.setContent(page);
    await tab.evaluate((s) => window.render(s), scale);
    const png = join(scratch, `bg-${scale}x.png`);
    const qpng = join(scratch, `bg-${scale}x.q.png`);
    const tif = join(scratch, `bg-${scale}x.tif`);
    await tab.screenshot({ path: png });
    await tab.close();

    execFileSync("pngquant", [
      "--quality=80-95",
      "--speed",
      "1",
      "--force",
      "--output",
      qpng,
      png,
    ]);
    execFileSync("sips", ["-s", "format", "tiff", qpng, "--out", tif], {
      stdio: "ignore",
    });
    if (scale === 2) {
      execFileSync(
        "sips",
        ["-s", "dpiWidth", "144", "-s", "dpiHeight", "144", tif],
        {
          stdio: "ignore",
        },
      );
    }
    tifs.push(tif);
  }
  await browser.close();

  execFileSync("tiffcp", ["-c", "zip:p9", ...tifs, OUT]);
  const bytes = statSync(OUT).size;
  console.log(`DMG background written to ${OUT} (${bytes} bytes)`);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
