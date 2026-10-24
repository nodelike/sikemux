import { describe, expect, it } from "vitest";
import { isLiveStatus, localDateTimeToIso } from "./shape";

describe("status", () => {
    it("treats queued and scheduled runs as live", () => {
        expect(isLiveStatus("running")).toBe(true);
        expect(isLiveStatus("Scheduled")).toBe(true);
        expect(isLiveStatus("queued")).toBe(true);
        expect(isLiveStatus("succeeded")).toBe(false);
        expect(isLiveStatus(null)).toBe(false);
    });
});

describe("time", () => {
    it("turns a datetime-local value into ISO with the local offset", () => {
        const iso = localDateTimeToIso("2026-09-25T14:30");
        expect(iso).toMatch(/^2026-09-25T14:30:00[+-]\d{2}:\d{2}$/);
        expect(Date.parse(iso!)).toBe(new Date(2026, 8, 25, 14, 30).getTime());
        expect(localDateTimeToIso("not a date")).toBeNull();
    });
});
