import { describe, expect, it } from "vitest";
import { SUMMARY_LIMIT, summaryJobs } from "../runStatus";
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

describe("summaryJobs", () => {
    const job = (id: number, conclusion = "success", checkRunId: string | null = String(id)) => ({
        id: String(id),
        name: `job ${id}`,
        status: "completed",
        conclusion,
        startedAt: null,
        completedAt: null,
        runner: null,
        url: null,
        checkRunId,
        steps: [],
    });

    it("asks for nothing while the run is still going", () => {
        expect(summaryJobs([job(1)], false, true)).toEqual([]);
    });

    it("skips jobs that could not have written one", () => {
        const found = summaryJobs([job(1), job(2, "skipped"), job(3, "success", null)], true, false);
        expect(found.map((each) => each.id)).toEqual(["1"]);
    });

    it("only loads the first few of a wide matrix until asked for the rest", () => {
        const wide = Array.from({ length: SUMMARY_LIMIT + 5 }, (_, index) => job(index + 1));
        expect(summaryJobs(wide, true, false)).toHaveLength(SUMMARY_LIMIT);
        expect(summaryJobs(wide, true, true)).toHaveLength(SUMMARY_LIMIT + 5);
    });
});
