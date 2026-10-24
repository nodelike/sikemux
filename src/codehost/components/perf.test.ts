import { describe, expect, it } from "vitest";
import { coarse } from "./hooks";

describe("coarse", () => {
    it("holds still within the minute, so a row reading 3h ago is not redrawn every second", () => {
        const base = Date.parse("2026-01-01T12:00:00Z");
        expect(coarse(base)).toBe(coarse(base + 59_000));
    });

    it("moves on once the minute does", () => {
        const base = Date.parse("2026-01-01T12:00:00Z");
        expect(coarse(base + 60_000)).toBeGreaterThan(coarse(base));
    });
});
