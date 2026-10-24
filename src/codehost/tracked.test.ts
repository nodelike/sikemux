import { describe, expect, it } from "vitest";
import { filterTracked, type TrackedItem } from "./tracked";

describe("filterTracked", () => {
    const item = (number: number, title: string, createdAt: string): TrackedItem => ({ kind: "issue", number, title, createdAt, url: "" });
    const items = [
        item(12, "Login crashes on start", "2026-01-03"),
        item(120, "Dark theme", "2026-01-02"),
        item(7, "Crash in login form", "2026-01-01"),
    ];

    it("matches digits against the start of a number", () => {
        expect(filterTracked(items, "12", 10).map((item) => item.number)).toEqual([12, 120]);
        expect(filterTracked(items, "2", 10)).toEqual([]);
    });

    it("matches every word against the title, in any order", () => {
        expect(filterTracked(items, "login crash", 10).map((item) => item.number)).toEqual([12, 7]);
        expect(filterTracked(items, "theme", 10).map((item) => item.number)).toEqual([120]);
    });

    it("offers the newest when nothing is typed yet, up to the limit", () => {
        expect(filterTracked(items, "", 2).map((item) => item.number)).toEqual([12, 120]);
    });
});
