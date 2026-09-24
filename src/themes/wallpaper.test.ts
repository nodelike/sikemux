import { describe, expect, it } from "vitest";
import { contrastRatio } from "../lib/themeContrast";
import { wallpaperTheme } from "./wallpaper";

function paint(size: number, pick: (x: number, y: number) => [number, number, number]): Uint8ClampedArray {
    const pixels = new Uint8ClampedArray(size * size * 4);
    for (let y = 0; y < size; y++)
        for (let x = 0; x < size; x++) {
            const at = (y * size + x) * 4;
            pixels.set([...pick(x, y), 255], at);
        }
    return pixels;
}

describe("wallpaperTheme", () => {
    it("takes a dark ground and its loudest colour from a night wallpaper", () => {
        const theme = wallpaperTheme(
            paint(64, (x, y) => (x > 40 && y > 40 ? [255, 60, 170] : [8, 10, 20])),
            "Night",
        );
        const [bg, , , ink, , , acc] = theme.chrome;
        expect(theme.dark).toBe(true);
        expect(contrastRatio(ink, bg)).toBeGreaterThanOrEqual(7);
        const [r, g, b] = [1, 3, 5].map((at) => parseInt(acc.slice(at, at + 2), 16));
        expect(r).toBeGreaterThan(g);
        expect(b).toBeGreaterThan(g);
    });

    it("turns a pale wallpaper into a light theme", () => {
        const theme = wallpaperTheme(
            paint(64, (x) => (x < 8 ? [40, 120, 200] : [238, 232, 220])),
            "Paper",
        );
        expect(theme.dark).toBe(false);
        expect(contrastRatio(theme.chrome[3], theme.chrome[0])).toBeGreaterThanOrEqual(7);
    });

    it("still builds a full palette from a wallpaper with no colour in it", () => {
        const theme = wallpaperTheme(
            paint(32, (x) => [x * 4, x * 4, x * 4]),
            "Grey",
        );
        expect(theme.terminal).toHaveLength(21);
        expect(theme.terminal.every((color) => /^#[0-9a-f]{6}$/.test(color))).toBe(true);
    });
});
