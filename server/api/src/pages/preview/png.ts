import { crc32, deflateSync } from "node:zlib";

/** One tone of red, green and blue, and how much it counts towards the palette. */
type Tone = [number, number, number, number, number];

interface Box {
  tones: Tone[];
  axis: 0 | 1 | 2;
  spread: number;
  score: number;
}

/** Tones under this are kept apart to the last step, so a dark glow fades without bands. */
const FINE = 48;
const level = (v: number) => (v < FINE ? v : FINE + ((v - FINE) >> 2));
const value = (l: number) => (l < FINE ? l : FINE + (l - FINE) * 4 + 1.5);
const toneOf = (px: Uint8Array, at: number) =>
  (level(px[at] ?? 0) << 14) |
  (level(px[at + 1] ?? 0) << 7) |
  level(px[at + 2] ?? 0);

function box(tones: Tone[]): Box {
  const low = [999, 999, 999];
  const high = [0, 0, 0];
  let count = 0;
  for (const tone of tones) {
    count += tone[3];
    for (let k = 0 as 0 | 1 | 2; k < 3; k++) {
      low[k] = Math.min(low[k] ?? 0, tone[k]);
      high[k] = Math.max(high[k] ?? 0, tone[k]);
    }
  }
  const spreads = high.map((top, k) => top - (low[k] ?? 0));
  const spread = Math.max(...spreads);
  const axis = spreads.indexOf(spread) as 0 | 1 | 2;
  return { tones, axis, spread, score: spread * count ** 0.6 };
}

/** Halves the boxes that vary most until there are 256, one for each colour of the palette. */
function boxesOf(tones: Tone[]): Box[] {
  const boxes = [box(tones)];
  while (boxes.length < 256) {
    const split = boxes.reduce<Box | null>(
      (best, next) =>
        next.spread > 0 && next.score > (best?.score ?? 0) ? next : best,
      null,
    );
    if (!split) break;
    const { tones, axis } = split;
    tones.sort((p, q) => p[axis] - q[axis]);
    const half = tones.reduce((sum, tone) => sum + tone[3], 0) / 2;
    let at = 0;
    for (let sum = 0; at < tones.length - 1 && sum < half; at++)
      sum += tones[at]?.[3] ?? 0;
    at = Math.max(1, at);
    boxes.splice(
      boxes.indexOf(split),
      1,
      box(tones.slice(0, at)),
      box(tones.slice(at)),
    );
  }
  return boxes;
}

function chunk(type: string, data: Buffer): Buffer {
  const named = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const out = Buffer.alloc(named.length + 8);
  out.writeUInt32BE(data.length, 0);
  named.copy(out, 4);
  out.writeUInt32BE(crc32(named), named.length + 4);
  return out;
}

/** RGB pixels as a PNG of 256 colours picked for them. */
export function encodePng(
  px: Uint8Array,
  width: number,
  height: number,
): Buffer {
  const counts = new Uint32Array(1 << 21);
  for (let at = 0; at < px.length; at += 3) {
    const tone = toneOf(px, at);
    counts[tone] = (counts[tone] ?? 0) + 1;
  }
  const tones: Tone[] = [];
  counts.forEach((count, tone) => {
    if (count)
      tones.push([
        tone >> 14,
        (tone >> 7) & 127,
        tone & 127,
        Math.sqrt(count),
        tone,
      ]);
  });
  const entry = new Uint8Array(1 << 21);
  const palette = Buffer.alloc(768);
  boxesOf(tones).forEach(({ tones }, at) => {
    const count = tones.reduce((sum, tone) => sum + tone[3], 0);
    for (let k = 0 as 0 | 1 | 2; k < 3; k++) {
      const sum = tones.reduce((sum, t) => sum + value(t[k]) * t[3], 0);
      palette[at * 3 + k] = Math.min(255, Math.round(sum / count));
    }
    for (const tone of tones) entry[tone[4]] = at;
  });
  const rows = Buffer.alloc((width + 1) * height);
  for (let y = 0, into = 0, at = 0; y < height; y++) {
    rows[into++] = 0;
    for (let x = 0; x < width; x++, at += 3)
      rows[into++] = entry[toneOf(px, at)] ?? 0;
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 3;
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", header),
    chunk("PLTE", palette),
    chunk("IDAT", deflateSync(rows, { level: 6 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
