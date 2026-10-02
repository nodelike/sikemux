import { describe, expect, it } from "vitest";
import { allowedSlot, clampTravel, dropForSlot, settleOffset, slideFor, slotAt, type TabBox } from "./tabDrag";

// Four 100px tabs side by side: a 0–100, b 100–200, c 200–300, d 300–400.
const boxes: TabBox[] = ["a", "b", "c", "d"].map((id, i) => ({ id, left: i * 100, right: i * 100 + 100 }));

describe("sliding a dragged tab through the strip", () => {
    it("passes a neighbour once its leading edge crosses the neighbour's middle", () => {
        expect(slotAt(boxes, 0, 40)).toBe(0); // right edge at 140, short of b's middle (150)
        expect(slotAt(boxes, 0, 60)).toBe(1);
        expect(slotAt(boxes, 3, -60)).toBe(2); // left edge at 240, past c's middle (250)
        expect(slotAt(boxes, 1, 0)).toBe(1);
    });

    it("reaches either end of the strip without leaving it", () => {
        expect(slotAt(boxes, 0, clampTravel(boxes, 0, 900))).toBe(3);
        expect(slotAt(boxes, 3, clampTravel(boxes, 3, -900))).toBe(0);
    });

    it("lets a wide tab pass a narrow one at the end", () => {
        const uneven: TabBox[] = [
            { id: "wide", left: 0, right: 200 },
            { id: "narrow", left: 200, right: 280 },
        ];
        expect(slotAt(uneven, 0, clampTravel(uneven, 0, 900))).toBe(1);
    });

    it("turns a slot into the drop that lands there", () => {
        expect(dropForSlot(boxes, 0, 2)).toEqual({ targetId: "c", placement: "after" });
        expect(dropForSlot(boxes, 3, 1)).toEqual({ targetId: "b", placement: "before" });
        expect(dropForSlot(boxes, 1, 1)).toBeNull();
    });

    it("opens the gap by sliding only the tabs between start and slot", () => {
        // a held over c: b and c slide left one width, d stays.
        expect([1, 2, 3].map((i) => slideFor(i, 0, 2, 100))).toEqual([-100, -100, 0]);
        // d held over b: b and c slide right, a stays.
        expect([0, 1, 2].map((i) => slideFor(i, 3, 1, 100))).toEqual([0, 100, 100]);
    });

    it("settles the dragged tab exactly into its slot", () => {
        expect(settleOffset(boxes, 0, 2)).toBe(200);
        expect(settleOffset(boxes, 3, 1)).toBe(-200);
        expect(settleOffset(boxes, 2, 2)).toBe(0);
    });

    it("stops at the last slot the owner allows", () => {
        const notPastC = (_s: string, target: string) => target !== "d";
        expect(allowedSlot(boxes, 0, 3, notPastC)).toBe(2);
        expect(allowedSlot(boxes, 0, 3, () => false)).toBe(0);
    });

    it("keeps the dragged tab within the strip", () => {
        expect(clampTravel(boxes, 1, 900)).toBe(200); // b's right edge stops at d's
        expect(clampTravel(boxes, 1, -900)).toBe(-100); // b's left edge stops at a's
    });
});
