import { describe, expect, it } from "vitest";
import { formatAgo, formatBytes, formatDuration } from "./format";

const NOW = Date.parse("2026-01-01T12:00:00Z");

describe("formatDuration", () => {
    it("writes a span in the units a build log uses", () => {
        expect(formatDuration(4_000)).toBe("4s");
        expect(formatDuration(72_000)).toBe("1m 12s");
        expect(formatDuration(3_780_000)).toBe("1h 3m");
        expect(formatDuration(null)).toBe("—");
    });

    it("keeps the precision a latency needs under a second", () => {
        expect(formatDuration(0.25)).toBe("250µs");
        expect(formatDuration(12.34)).toBe("12.3ms");
        expect(formatDuration(1_530)).toBe("1.5s");
        expect(formatDuration(42_000)).toBe("42s");
    });
});

describe("formatAgo", () => {
    it("picks the one unit that fits", () => {
        expect(formatAgo("2026-01-01T11:59:30Z", NOW)).toBe("now");
        expect(formatAgo("2026-01-01T11:30:00Z", NOW)).toBe("30m ago");
        expect(formatAgo("2026-01-01T06:00:00Z", NOW)).toBe("6h ago");
        expect(formatAgo("2025-12-29T12:00:00Z", NOW)).toBe("3d ago");
        expect(formatAgo("2025-10-01T12:00:00Z", NOW)).toBe("3mo ago");
    });

    it("says nothing about a time it does not have", () => {
        expect(formatAgo(null, NOW)).toBe("—");
        expect(formatAgo("whenever", NOW)).toBe("—");
    });
});

describe("formatBytes", () => {
    it("writes a size the way a download shelf does", () => {
        expect(formatBytes(0)).toBe("0 B");
        expect(formatBytes(512)).toBe("512 B");
        expect(formatBytes(2048)).toBe("2.0 KB");
        expect(formatBytes(15_360)).toBe("15 KB");
        expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
        expect(formatBytes(3 * 1024 * 1024 * 1024)).toBe("3.0 GB");
    });

    it("stops at terabytes rather than inventing a unit", () => {
        expect(formatBytes(5 * 1024 ** 5)).toBe("5120 TB");
    });
});
