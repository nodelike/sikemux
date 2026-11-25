import { createHash } from "node:crypto";

export const WIDTH = 1200;
export const HEIGHT = 630;

type Rgb = [number, number, number];
type Roll = () => number;

interface Scene {
  tint: Rgb;
  alt: Rgb;
  bright: Rgb;
  deep: Rgb;
}

interface Place {
  x: number;
  y: number;
  radius: number;
}

const rgb = (hex: string): Rgb => [
  parseInt(hex.slice(1, 3), 16),
  parseInt(hex.slice(3, 5), 16),
  parseInt(hex.slice(5, 7), 16),
];
const scene = (
  tint: string,
  alt: string,
  bright: string,
  deep: string,
): Scene => ({
  tint: rgb(tint),
  alt: rgb(alt),
  bright: rgb(bright),
  deep: rgb(deep),
});
const PURPLE = scene("#a277ff", "#ff6ac1", "#f6f1ff", "#4a2ca8");
const SCENES: Scene[] = [
  PURPLE,
  scene("#82d9ff", "#a277ff", "#f2fbff", "#1f5f9c"),
  scene("#ff6ac1", "#ffca85", "#fff0f8", "#8c1f63"),
  scene("#61ffca", "#82d9ff", "#eefff9", "#0f7a5c"),
  scene("#ffca85", "#ff6ac1", "#fff8ec", "#8f5a1c"),
  scene("#61ffca", "#a277ff", "#eefff9", "#2b5f8c"),
  scene("#a277ff", "#ffca85", "#f6f1ff", "#4a2ca8"),
];
const GROUND: Rgb = [9, 9, 11];
const GRAINS = 130_000;
const DUST = 26_000;
const BOKEH = 34;
/** How far out the figure starts, as a share of its radius, so the mark in its middle stays clear. */
const INNER = 0.24;

const TAU = Math.PI * 2;
const clamp = (v: number, low = 0, high = 1) =>
  Math.max(low, Math.min(high, v));
const smooth = (from: number, to: number, v: number) => {
  const t = clamp((v - from) / (to - from));
  return t * t * (3 - 2 * t);
};
const mix = (a: Rgb, b: Rgb, t: number): Rgb => [
  a[0] + (b[0] - a[0]) * t,
  a[1] + (b[1] - a[1]) * t,
  a[2] + (b[2] - a[2]) * t,
];
const fraction = (v: number) => v - Math.floor(v);
function speckle(x: number, y: number): number {
  const u = fraction(x * 123.34);
  const v = fraction(y * 456.21);
  const k = u * (u + 45.32) + v * (v + 45.32);
  return fraction((u + k) * (v + k));
}
function cloud(x: number, y: number): number {
  const i = Math.floor(x);
  const j = Math.floor(y);
  const f = smooth(0, 1, x - i);
  const g = smooth(0, 1, y - j);
  const a = speckle(i, j);
  const b = speckle(i + 1, j);
  const c = speckle(i, j + 1);
  const d = speckle(i + 1, j + 1);
  return (a + (b - a) * f) * (1 - g) + (c + (d - c) * f) * g;
}

/** The same numbers in the same order for an id, every time. */
function seeded(id: string): { roll: Roll; scene: Scene } {
  const digest = createHash("sha256").update(id).digest();
  let state = digest.readInt32LE(0);
  const roll = () => {
    state = (state + 0x6d2b79f5) | 0;
    let v = Math.imul(state ^ (state >>> 15), 1 | state);
    v = (v + Math.imul(v ^ (v >>> 7), 61 | v)) ^ v;
    return ((v ^ (v >>> 14)) >>> 0) / 4294967296;
  };
  const scene = SCENES[digest.readUInt8(8) % SCENES.length] ?? PURPLE;
  return { roll, scene };
}

class Canvas {
  readonly px = new Float32Array(WIDTH * HEIGHT * 3);

  constructor() {
    for (let i = 0; i < WIDTH * HEIGHT; i++) this.px.set(GROUND, i * 3);
  }

  blend(x: number, y: number, colour: Rgb, alpha: number) {
    const at = (y * WIDTH + x) * 3;
    for (let k = 0; k < 3; k++) {
      const under = this.px[at + k] ?? 0;
      this.px[at + k] = under + ((colour[k] ?? 0) - under) * alpha;
    }
  }

  /** Blends `colour` into every pixel within `reach` of a point, as strongly as `alpha` says. */
  spot(
    x: number,
    y: number,
    reach: number,
    colour: Rgb,
    alpha: (distance: number) => number,
    centre = 0.5,
  ) {
    const bottom = Math.min(HEIGHT - 1, Math.ceil(y + reach));
    const right = Math.min(WIDTH - 1, Math.ceil(x + reach));
    for (let v = Math.max(0, Math.floor(y - reach)); v <= bottom; v++)
      for (let u = Math.max(0, Math.floor(x - reach)); u <= right; u++) {
        const strength = alpha(Math.hypot(u + centre - x, v + centre - y));
        if (strength > 0) this.blend(u, v, colour, strength);
      }
  }

  grain(x: number, y: number, radius: number, colour: Rgb, alpha: number) {
    const whole = alpha * Math.min(1, radius / 0.6);
    this.spot(
      x,
      y,
      radius + 1,
      colour,
      (distance) => clamp(radius - distance + 0.5) * whole,
    );
  }
}

/** A soft glow behind the figure, worked out on a coarse grid and smoothed between its points. */
function glow(
  canvas: Canvas,
  { tint, alt, deep }: Scene,
  at: Place,
  roll: Roll,
) {
  const CELL = 10;
  const POWER = 0.15;
  const columns = Math.ceil(WIDTH / CELL) + 1;
  const rows = Math.ceil(HEIGHT / CELL) + 1;
  const grid = new Float32Array(columns * rows * 3);
  const ox = roll() * 90;
  const oy = roll() * 90;
  const near = (x: number, y: number, span: number) =>
    Math.exp(-(x * x + y * y) / (span * span));
  for (let j = 0; j < rows; j++)
    for (let i = 0; i < columns; i++) {
      const x = i * CELL;
      const y = j * CELL;
      const thick = cloud(x / 300 + ox, y / 300 + oy);
      const far = cloud(x / 420 + oy + 31, y / 420 + ox);
      const dx = x - at.x;
      const dy = y - at.y;
      const wide = near(dx, dy, at.radius * 1.9) * (0.5 + thick);
      const tight = near(dx, dy, at.radius * 0.9) * (0.6 + 0.5 * thick);
      const left = near(x - WIDTH * 0.1, y - HEIGHT * 1.05, 520) * (0.3 + far);
      const right = near(x - WIDTH * 0.95, y, 480) * (0.25 + far);
      for (let k = 0 as 0 | 1 | 2; k < 3; k++)
        grid[(j * columns + i) * 3 + k] =
          POWER *
          ((deep[k] * 0.5 + tint[k] * 0.1) * wide +
            tint[k] * 0.16 * tight +
            alt[k] * 0.12 * left +
            deep[k] * 0.3 * right);
    }
  const cell = (index: number) => grid[index] ?? 0;
  for (let y = 0; y < HEIGHT; y++)
    for (let x = 0; x < WIDTH; x++) {
      const i = Math.floor(x / CELL);
      const j = Math.floor(y / CELL);
      const f = x / CELL - i;
      const g = y / CELL - j;
      const from = (j * columns + i) * 3;
      const below = from + columns * 3;
      const into = (y * WIDTH + x) * 3;
      for (let k = 0; k < 3; k++) {
        const top = cell(from + k) * (1 - f) + cell(from + 3 + k) * f;
        const low = cell(below + k) * (1 - f) + cell(below + 3 + k) * f;
        canvas.px[into + k] =
          (canvas.px[into + k] ?? 0) + top * (1 - g) + low * g;
      }
    }
}

/** Loose dust around the figure, thick where the cloud is. */
function dust(
  canvas: Canvas,
  { tint, alt, bright, deep }: Scene,
  at: Place,
  roll: Roll,
) {
  const ox = roll() * 70;
  const oy = roll() * 70;
  for (let i = 0; i < DUST; i++) {
    const x = roll() * WIDTH;
    const y = roll() * HEIGHT;
    const u = x / 150 + ox;
    const v = y / 150 + oy;
    const thick = cloud(
      u + 2 * cloud(u * 0.7, v * 0.7),
      v + 2 * cloud(u * 0.7 + 9, v * 0.7 + 9),
    );
    const away = Math.hypot(x - at.x, y - at.y);
    const weight =
      0.5 *
      smooth(at.radius * 0.95, at.radius * 1.15, away) *
      clamp(1.15 - away / 780);
    const chance = weight * smooth(0.2, 0.85, thick) * 1.3;
    if (roll() > chance) continue;
    const spark = roll();
    const warm = cloud(x / 330 + ox, y / 330 + oy);
    const other = cloud(x / 210 + oy, y / 210 + 17);
    let colour = mix(
      deep,
      tint,
      smooth(0.15, 0.6, warm + 0.2 * (roll() - 0.5)),
    );
    colour = mix(colour, alt, smooth(0.5, 0.74, other));
    if (spark > 0.96) colour = mix(colour, bright, 0.6);
    const radius = 0.6 + 0.9 * roll() ** 4;
    canvas.grain(x, y, radius, colour, 0.25 + 0.5 * clamp(chance));
  }
}

function shade(
  { tint, alt, bright, deep }: Scene,
  lit: number,
  rim: number,
): Rgb {
  const body =
    lit > 0.5
      ? mix(tint, bright, (lit - 0.5) * 1.7)
      : mix(mix(deep, tint, 0.35), tint, lit * 2);
  return mix(body, alt, clamp(rim, 0, 0.85));
}

/** Sand on a ringing plate: grains gather where a wave of the id's own shape stands still. */
function figure(canvas: Canvas, scene: Scene, at: Place, roll: Roll) {
  const folds = 3 + Math.floor(roll() * 5);
  const finer = folds * (1 + Math.floor(roll() * 2));
  const a = 2.6 + roll() * 2.4;
  const b = 4.2 + roll() * 3.6;
  const second = 0.55 + roll() * 0.6;
  const phases = [roll() * 6.28, roll() * 6.28, roll() * 6.28] as const;
  const third = 0.25 + roll() * 0.5;
  const twist = (roll() - 0.5) * 0.9;
  const wave = (r: number, t: number) =>
    Math.cos(a * Math.PI * r + phases[0]) * Math.cos(folds * t) +
    second * Math.cos(b * Math.PI * r + phases[1]) * Math.cos(2 * finer * t) +
    third * Math.sin((a + b) * Math.PI * r * 0.6 + phases[2]);
  const turn = roll() * 6.28;
  dust(canvas, scene, at, roll);
  for (let i = 0; i < GRAINS; i++) {
    const r = Math.sqrt(roll());
    const t = roll() * TAU;
    if (r < INNER) continue;
    const still = wave(r, t + twist * r);
    const line = Math.exp(-((still / 0.15) ** 2));
    const soft = Math.exp(-((still / 0.55) ** 2)) * 0.16 + 0.018;
    const edge = smooth(1, 0.9, r) * smooth(INNER, INNER + 0.05, r);
    if (roll() > (line + soft) * edge * 0.8) continue;
    const angle = t + turn;
    const x = at.x + Math.cos(angle) * r * at.radius + (roll() - 0.5) * 1.4;
    const y = at.y + Math.sin(angle) * r * at.radius + (roll() - 0.5) * 1.4;
    const u = (x - at.x) / at.radius;
    const v = (y - at.y) / at.radius;
    const lit = clamp(0.5 - u * 0.28 - v * 0.45 + (roll() - 0.5) * 0.3);
    const rim = Math.max(0, (u + v) * 0.55);
    const radius = 0.75 + 0.75 * roll() ** 2;
    const alpha = line > 0.4 ? 0.62 + 0.38 * lit : 0.4;
    canvas.grain(x, y, radius, shade(scene, lit, rim), alpha);
  }
}

function bokeh(canvas: Canvas, { tint, alt }: Scene, at: Place, roll: Roll) {
  for (let i = 0; i < BOKEH; i++) {
    const x = roll() * WIDTH;
    const y = roll() * HEIGHT;
    const q = roll();
    if (Math.hypot(x - at.x, y - at.y) <= at.radius * 1.05) continue;
    const reach = (3 + 9 * q * q) * 2.2;
    const colour = mix(tint, alt, roll());
    const alpha = (distance: number) =>
      (0.1 + 0.1 * (1 - q)) * (1 - smooth(0.25, 1, distance / reach));
    canvas.spot(x, y, reach, colour, alpha, 0);
  }
}

/** The picture behind a share's words: a figure only its id makes, as RGB values from 0 to 255. */
export function drawNote(id: string, at: Place): Float32Array {
  const { roll, scene } = seeded(id);
  const canvas = new Canvas();
  glow(canvas, scene, at, roll);
  figure(canvas, scene, at, roll);
  bokeh(canvas, scene, at, roll);
  return canvas.px;
}
