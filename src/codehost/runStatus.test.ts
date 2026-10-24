import { describe, expect, it } from "vitest";
import { elapsedMs } from "./runStatus";

const NOW = Date.parse("2026-01-01T12:00:00Z");

describe("elapsedMs", () => {
    it("measures a finished span between its own two ends", () => {
        expect(elapsedMs("2026-01-01T11:58:00Z", "2026-01-01T11:59:30Z", NOW)).toBe(90_000);
    });

    it("measures an unfinished span up to now", () => {
        expect(elapsedMs("2026-01-01T11:59:00Z", null, NOW)).toBe(60_000);
    });

    it("has nothing to measure without a start, or from nonsense", () => {
        expect(elapsedMs(null, null, NOW)).toBeNull();
        expect(elapsedMs("not a time", null, NOW)).toBeNull();
        expect(elapsedMs("2026-01-01T11:59:00Z", "not a time", NOW)).toBeNull();
    });

    it("never reports a negative span from a clock that disagrees", () => {
        expect(elapsedMs("2026-01-01T12:05:00Z", null, NOW)).toBe(0);
    });
});
