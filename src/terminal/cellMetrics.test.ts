import { describe, expect, it } from "vitest";
import { cellWidthCorrection, lineHeightCorrection } from "./cellMetrics";

describe("cellWidthCorrection", () => {
    it("widens the cell back when JetBrains Mono lands between pixels", () => {
        // 13px at 0.6em is 7.8 css pixels, which WebGL would floor to 15 of 15.6.
        expect(cellWidthCorrection(7.8, 2)).toBe(1);
        expect(cellWidthCorrection(7.8, 1)).toBe(1);
    });

    it("leaves a cell that already lands on a pixel alone", () => {
        expect(cellWidthCorrection(9, 2)).toBe(0);
        expect(cellWidthCorrection(7.5, 2)).toBe(0);
    });

    it("keeps the cell at the nearer pixel rather than always widening", () => {
        expect(cellWidthCorrection(7.7, 3)).toBe(0); // 23.1 is already close enough to 23
        expect(cellWidthCorrection(7.6, 3)).toBe(1); // 22.8 belongs at 23, not 22
    });

    it.each([0, -1, Number.NaN, Number.POSITIVE_INFINITY])("makes no correction for a char width of %j", (width) => {
        expect(cellWidthCorrection(width, 2)).toBe(0);
    });

    it.each([0, -2, Number.NaN])("makes no correction for a pixel ratio of %j", (ratio) => {
        expect(cellWidthCorrection(7.8, ratio)).toBe(0);
    });
});

describe("lineHeightCorrection", () => {
    const rowPixels = (fontSize: number, charHeight: number, ratio: number) =>
        Math.floor(Math.ceil(charHeight * ratio) * lineHeightCorrection(fontSize, charHeight, ratio));

    it("brings WebKit's short 13px row up to the 34 device pixels Ghostty uses", () => {
        expect(rowPixels(13, 15, 2)).toBe(34);
        expect(rowPixels(13, 15, 1)).toBe(17);
    });

    it("follows the font size", () => {
        expect(rowPixels(16, 18, 2)).toBe(42);
        expect(rowPixels(11, 13, 2)).toBe(29);
    });

    it("never shrinks a row that is already tall enough", () => {
        expect(lineHeightCorrection(13, 20, 2)).toBe(1);
    });

    it.each([0, -1, Number.NaN])("makes no correction for a char height of %j", (height) => {
        expect(lineHeightCorrection(13, height, 2)).toBe(1);
    });
});
