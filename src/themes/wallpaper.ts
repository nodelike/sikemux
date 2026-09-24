import { ghosttyTheme, type ThemeColours } from "./ghostty";

interface Lch {
    l: number;
    c: number;
    h: number;
}

interface Cluster extends Lch {
    count: number;
}

const toLinear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const fromLinear = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);

function rgbToLch(r: number, g: number, b: number): Lch {
    const [lr, lg, lb] = [r, g, b].map((c) => toLinear(c / 255));
    const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
    const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
    const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
    const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
    const a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    const bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    return { l: L, c: Math.hypot(a, bb), h: ((Math.atan2(bb, a) * 180) / Math.PI + 360) % 360 };
}

function lchToLinear({ l, c, h }: Lch): number[] {
    const a = c * Math.cos((h * Math.PI) / 180),
        b = c * Math.sin((h * Math.PI) / 180);
    const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
    const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
    const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
    return [
        4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
        -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
        -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
    ];
}

/** Hex for an OKLCH colour, giving up chroma until it fits in sRGB. */
function hex(color: Lch): string {
    let { c } = color;
    let rgb = lchToLinear({ ...color, c });
    while (c > 0 && rgb.some((v) => v < -0.0001 || v > 1.0001)) {
        c = Math.max(0, c - 0.005);
        rgb = lchToLinear({ ...color, c });
    }
    return rgb
        .map((v) =>
            Math.round(fromLinear(Math.min(1, Math.max(0, v))) * 255)
                .toString(16)
                .padStart(2, "0"),
        )
        .join("");
}

const hueDistance = (a: number, b: number) => Math.abs(((((a - b) % 360) + 540) % 360) - 180);

/** Splits the pixels along their widest channel until there are `depth` levels of boxes, like Aether's median cut. */
function medianCut(pixels: number[][], depth: number): number[][][] {
    if (depth === 0 || pixels.length < 2) return [pixels];
    const spans = [0, 1, 2].map((channel) => {
        let min = 255,
            max = 0;
        for (const p of pixels) {
            if (p[channel] < min) min = p[channel];
            if (p[channel] > max) max = p[channel];
        }
        return max - min;
    });
    const channel = spans.indexOf(Math.max(...spans));
    const sorted = [...pixels].sort((a, b) => a[channel] - b[channel]);
    const middle = sorted.length >> 1;
    return [...medianCut(sorted.slice(0, middle), depth - 1), ...medianCut(sorted.slice(middle), depth - 1)];
}

function clusters(rgba: ArrayLike<number>): Cluster[] {
    const total = rgba.length / 4;
    const stride = Math.max(1, Math.floor(total / 24000));
    const pixels: number[][] = [];
    for (let i = 0; i < total; i += stride) {
        const at = i * 4;
        if (rgba[at + 3] >= 128) pixels.push([rgba[at], rgba[at + 1], rgba[at + 2]]);
    }
    if (pixels.length === 0) throw new Error("the wallpaper has no visible pixels");
    return medianCut(pixels, 5)
        .filter((box) => box.length > 0)
        .map((box) => {
            const mean = [0, 1, 2].map((channel) => box.reduce((sum, p) => sum + p[channel], 0) / box.length);
            return { ...rgbToLch(mean[0], mean[1], mean[2]), count: box.length };
        });
}

const ANSI_HUES = [29, 100, 142, 195, 262, 328];

/** Turns a wallpaper's pixels into a terminal palette and hands it to the same derivation Ghostty themes use. */
export function wallpaperTheme(rgba: ArrayLike<number>, name: string): ThemeColours {
    const found = clusters(rgba);
    const population = found.reduce((sum, c) => sum + c.count, 0);
    const brightness = found.reduce((sum, c) => sum + c.l * c.count, 0) / population;
    const dark = brightness < 0.62;
    const vivid = found.filter((c) => c.c >= 0.04 && c.l > 0.2 && c.l < 0.95);
    const byWeight = (c: Cluster) => c.count * c.c;
    const lead = [...vivid].sort((a, b) => byWeight(b) - byWeight(a))[0];
    const sizeable = found.filter((c) => c.count >= population * 0.05);
    const ground = [...(sizeable.length ? sizeable : found)].sort((a, b) => (dark ? a.l - b.l : b.l - a.l))[0];
    const tint = { h: ground.c >= 0.015 ? ground.h : (lead?.h ?? 270), c: Math.min(ground.c, 0.035) };
    const typicalChroma = vivid.length ? vivid.map((c) => c.c).sort((a, b) => a - b)[vivid.length >> 1] : 0.1;
    const chroma = Math.min(0.17, Math.max(0.08, typicalChroma));

    const neutral = (l: number, c = tint.c) => hex({ l, c, h: tint.h });
    const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
    const bg = dark ? neutral(clamp(ground.l, 0.14, 0.22)) : neutral(clamp(ground.l, 0.95, 0.985), Math.min(tint.c, 0.015));
    const fg = dark ? neutral(0.91, Math.min(tint.c, 0.02)) : neutral(0.3);
    const accentHue = lead?.h ?? tint.h;
    const accent = hex({ l: dark ? 0.76 : 0.54, c: lead ? Math.min(0.2, Math.max(0.1, lead.c)) : 0.08, h: accentHue });

    const colours = ANSI_HUES.map((target) => {
        const match = vivid.filter((c) => hueDistance(c.h, target) <= 32).sort((a, b) => b.count - a.count)[0];
        return { h: match?.h ?? target, c: match ? Math.min(0.2, Math.max(0.09, match.c)) : chroma };
    });
    const normal = colours.map(({ h, c }) => hex({ l: dark ? 0.72 : 0.54, c, h }));
    const bright = colours.map(({ h, c }) => hex({ l: dark ? 0.8 : 0.47, c: Math.min(0.22, c * 1.1), h }));
    const ansi = [
        neutral(dark ? 0.3 : 0.32),
        ...normal,
        neutral(dark ? 0.82 : 0.86),
        neutral(dark ? 0.52 : 0.58),
        ...bright,
        neutral(dark ? 0.96 : 0.95),
    ];
    const selection = hex({ l: dark ? 0.34 : 0.87, c: Math.min(0.08, chroma), h: accentHue });
    const encoded = [bg, fg, accent, bg, selection, ...ansi].join(",");
    return ghosttyTheme(name, encoded, `#${accent}`);
}

export async function wallpaperPixels(dataUrl: string, size = 160): Promise<Uint8ClampedArray> {
    const image = new Image();
    image.src = dataUrl;
    await image.decode();
    const scale = Math.min(1, size / Math.max(image.naturalWidth, image.naturalHeight));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
    const context = canvas.getContext("2d");
    if (!context) throw new Error("could not read the wallpaper's pixels");
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    return context.getImageData(0, 0, canvas.width, canvas.height).data;
}
