import { describe, expect, it } from "vitest";
import type { DashboardPanel } from "../api";
import { formatValue } from "./charts";
import { isHeading, placement } from "./DashboardView";

const panel = (layout: DashboardPanel["layout"]): DashboardPanel => ({
    id: "p",
    title: "p",
    kind: "graph",
    unit: "",
    layout,
    drawable: true,
    query: null,
});

describe("dashboard layout", () => {
    it("keeps each panel to exactly the rows it was saved with", () => {
        expect(placement(panel({ x: 0, y: 0, w: 12, h: 1 }))).toEqual({ gridColumn: "1 / span 12", gridRow: "1 / span 1" });
        expect(placement(panel({ x: 8, y: 1, w: 4, h: 3 }))).toEqual({ gridColumn: "9 / span 4", gridRow: "2 / span 3" });
    });

    it("never lets a panel run past the twelfth column", () => {
        expect(placement(panel({ x: 10, y: 0, w: 6, h: 3 })).gridColumn).toBe("11 / span 2");
    });

    it("treats a panel one row tall as a section heading", () => {
        expect(isHeading(panel({ x: 0, y: 0, w: 12, h: 1 }))).toBe(true);
        expect(isHeading(panel({ x: 0, y: 1, w: 2, h: 3 }))).toBe(false);
    });
});

describe("formatValue", () => {
    it("drops trailing zeros from small values", () => {
        expect(formatValue(0.3, "percent")).toBe("0.3%");
        expect(formatValue(2.5, "")).toBe("2.5");
        expect(formatValue(0.1234, "")).toBe("0.12");
        expect(formatValue(1284, "")).toBe("1284");
    });
});
