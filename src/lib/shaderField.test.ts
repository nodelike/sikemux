import { describe, expect, it } from "vitest";
import { mountShaderField, shaderFieldCount, shaderFieldDiagnostics, shaderFieldPixelCap, unmountShaderField } from "./shaderField";

/*
 * The runtime picks a canvas size by taking the host's device pixels and then
 * dividing them down until they fit the cap, which is the same arithmetic as
 * `handleResize` in `@paper-design/shaders`. Reproduced here so a change in the
 * cap is checked against the scale it actually produces rather than against a
 * number someone wrote down.
 */
function renderScale(cssWidth: number, cssHeight: number, devicePixelRatio: number): number {
    const targetWidth = cssWidth * devicePixelRatio;
    const targetHeight = cssHeight * devicePixelRatio;
    const cap = shaderFieldPixelCap(cssWidth, cssHeight);
    const fit = Math.min(1, Math.sqrt(cap) / Math.sqrt(targetWidth * targetHeight));
    return Math.round(targetWidth * fit) / cssWidth;
}

/*
 * jsdom has no WebGL, which is the same situation as a machine whose driver is
 * blocklisted. The contract under test is that this is silent: a field is
 * decoration, so a caller must never have to know whether one arrived.
 */
describe("shaderField", () => {
    it("stays silent and explains itself when WebGL is unavailable", () => {
        const host = document.createElement("span");
        document.body.append(host);

        mountShaderField(host, "ambient");

        expect(shaderFieldCount()).toBe(0);
        expect(host.dataset.shaderField).toBeUndefined();
        const report = shaderFieldDiagnostics();
        expect(report.live).toBe(0);
        expect(report.webgl2).toBe(false);
        expect(report.lastRefusal).toBe("no webgl2 context");

        expect(() => unmountShaderField(host)).not.toThrow();
        expect(host.querySelector("canvas")).toBeNull();
        host.remove();
    });

    /*
     * The seventh ShaderMount argument is a floor, not a cap, so a Retina
     * machine used to paint four pixels for every one on screen no matter what
     * was passed there. The cap is what actually holds the fill down.
     */
    it("holds a Retina screen to one shader pixel per CSS pixel", () => {
        expect(renderScale(1440, 900, 2)).toBeCloseTo(1, 5);
        expect(renderScale(1440, 900, 3)).toBeCloseTo(1, 5);
    });

    it("leaves a non-Retina screen alone rather than upscaling it", () => {
        expect(renderScale(1440, 900, 1)).toBeCloseTo(1, 5);
    });

    it("caps by area, so a reshaped host keeps the same fill cost", () => {
        expect(shaderFieldPixelCap(1440, 900)).toBe(shaderFieldPixelCap(900, 1440));
        expect(shaderFieldPixelCap(1440, 900)).toBe(1_296_000);
    });

    // A host measured before layout has no area, and a cap of zero would mean
    // "render nothing forever" rather than "ask again once it has a size".
    it("never caps a field down to nothing", () => {
        expect(shaderFieldPixelCap(0, 0)).toBe(1);
        expect(shaderFieldPixelCap(0.2, 0.2)).toBe(1);
    });
});
