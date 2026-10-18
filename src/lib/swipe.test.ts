import { describe, expect, it } from "vitest";
import { claimsWheel, dragOffset } from "./swipe";
import type { PaneScroller } from "./swipe";

const plain: PaneScroller = { overflowX: "visible", scrollWidth: 100, clientWidth: 100, scrollLeft: 0 };
const scroller = (over: Partial<PaneScroller> = {}): PaneScroller => ({
    overflowX: "auto",
    scrollWidth: 500,
    clientWidth: 200,
    scrollLeft: 100,
    ...over,
});

const bothWays = { hasPrevious: true, hasNext: true };
const firstScreen = { hasPrevious: false, hasNext: true };
const lastScreen = { hasPrevious: true, hasNext: false };

describe("claimsWheel", () => {
    it("takes a flat sideways gesture over nothing that scrolls", () => {
        expect(claimsWheel([plain, plain], 40, 2)).toBe(true);
    });

    /*
     * A gesture going mostly down the screen is somebody scrolling a pane with a
     * little sideways drift, not a swipe between screens.
     */
    it("leaves a gesture that is mostly vertical alone", () => {
        expect(claimsWheel([plain], 20, 20)).toBe(false);
        expect(claimsWheel([plain], 20, 14)).toBe(false);
        expect(claimsWheel([plain], 20, 12)).toBe(true);
    });

    it("leaves the gesture to a pane that can still scroll the way it is going", () => {
        expect(claimsWheel([plain, scroller()], 40, 0)).toBe(false);
        expect(claimsWheel([scroller(), plain], -40, 0)).toBe(false);
    });

    /*
     * The decision is made once, on the gesture's first event, so a pane already
     * at its edge never gets the gesture and a pane that reaches its edge
     * mid-swipe keeps it — the caller latches what this returns.
     */
    it("takes the gesture when the pane is already at the edge it is heading for", () => {
        expect(claimsWheel([scroller({ scrollLeft: 300 })], 40, 0)).toBe(true);
        expect(claimsWheel([scroller({ scrollLeft: 0 })], -40, 0)).toBe(true);
        expect(claimsWheel([scroller({ scrollLeft: 0 })], 40, 0)).toBe(false);
    });

    it("ignores a pane whose content fits, however it declares its overflow", () => {
        expect(claimsWheel([scroller({ scrollWidth: 200 })], 40, 0)).toBe(true);
        expect(claimsWheel([scroller({ overflowX: "hidden" })], 40, 0)).toBe(true);
    });
});

describe("dragOffset", () => {
    it("follows the finger one for one up to the next screen", () => {
        expect(dragOffset(0.4, bothWays)).toBeCloseTo(0.4);
        expect(dragOffset(-0.4, bothWays)).toBeCloseTo(-0.4);
        expect(dragOffset(1, bothWays)).toBeCloseTo(1);
    });

    /* One swipe moves one screen, so the screen after the next is never uncovered. */
    it("resists past the next screen", () => {
        expect(dragOffset(1.8, bothWays)).toBeGreaterThan(1);
        expect(dragOffset(1.8, bothWays)).toBeLessThan(1.15);
        expect(dragOffset(-1.8, bothWays)).toBeLessThan(-1);
        expect(dragOffset(-1.8, bothWays)).toBeGreaterThan(-1.15);
    });

    /*
     * Past the last screen there is nothing to uncover, so the track gives a
     * little and then stops giving rather than sliding off into blank stage.
     */
    it("resists past the ends of the session", () => {
        expect(dragOffset(0.4, lastScreen)).toBeLessThan(0.15);
        expect(dragOffset(0.4, lastScreen)).toBeGreaterThan(0);
        expect(dragOffset(50, lastScreen)).toBeLessThan(0.15);
        expect(dragOffset(-0.4, firstScreen)).toBeGreaterThan(-0.15);
        expect(dragOffset(-0.4, firstScreen)).toBeLessThan(0);
    });

    /*
     * All of the give spent in the first fraction of the pull is a stop, not a
     * wall: the rest of the finger travel moves nothing and reads as broken.
     */
    it("spreads the give over a screen of pull rather than spending it at once", () => {
        expect(dragOffset(0.25, lastScreen)).toBeLessThan(0.075);
        expect(dragOffset(1.5, lastScreen)).toBeGreaterThan(0.12);
        expect(dragOffset(1.5, lastScreen)).toBeGreaterThan(dragOffset(0.5, lastScreen));
    });

    it("still runs free towards the end that has a screen", () => {
        expect(dragOffset(0.4, firstScreen)).toBeCloseTo(0.4);
        expect(dragOffset(-0.4, lastScreen)).toBeCloseTo(-0.4);
    });
});
