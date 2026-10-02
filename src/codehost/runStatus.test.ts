import { describe, expect, it } from "vitest";
import type { Job, Step } from "./api";
import {
    elapsedMs,
    eventLabel,
    failedStep,
    formatAgo,
    formatDuration,
    checksSummary,
    isUnfinished,
    jobsSummary,
    outcomeOf,
    statusParam,
    watchIsNewer,
} from "./runStatus";

const NOW = Date.parse("2026-01-01T12:00:00Z");

const step = (name: string, status: string, conclusion: string | null): Step => ({
    number: 1,
    name,
    status,
    conclusion,
    startedAt: null,
    completedAt: null,
});

const job = (status: string, conclusion: string | null, steps: Step[] = []): Job => ({
    id: "1",
    name: "build",
    status,
    conclusion,
    startedAt: null,
    completedAt: null,
    runner: null,
    url: null,
    checkRunId: null,
    steps,
});

describe("outcomeOf", () => {
    it("reads a finished run by its conclusion", () => {
        expect(outcomeOf({ status: "completed", conclusion: "success" })).toBe("success");
        expect(outcomeOf({ status: "completed", conclusion: "failure" })).toBe("failure");
        expect(outcomeOf({ status: "completed", conclusion: "timed_out" })).toBe("failure");
        expect(outcomeOf({ status: "completed", conclusion: "startup_failure" })).toBe("failure");
        expect(outcomeOf({ status: "completed", conclusion: "cancelled" })).toBe("cancelled");
        expect(outcomeOf({ status: "completed", conclusion: "skipped" })).toBe("skipped");
    });

    it("reads a run still going by its status", () => {
        expect(outcomeOf({ status: "in_progress", conclusion: null })).toBe("running");
        expect(outcomeOf({ status: "queued", conclusion: null })).toBe("queued");
        expect(outcomeOf({ status: "waiting", conclusion: null })).toBe("blocked");
    });

    it("does not guess at a state it has not seen", () => {
        expect(outcomeOf({ status: "completed", conclusion: null })).toBe("unknown");
        expect(outcomeOf({ status: "something_new", conclusion: null })).toBe("unknown");
    });

    it("counts queued, running and waiting for approval as unfinished, and nothing that has completed", () => {
        expect(isUnfinished({ status: "in_progress", conclusion: null })).toBe(true);
        expect(isUnfinished({ status: "queued", conclusion: null })).toBe(true);
        expect(isUnfinished({ status: "waiting", conclusion: null })).toBe(true);
        expect(isUnfinished({ status: "completed", conclusion: "success" })).toBe(false);
        expect(isUnfinished({ status: "completed", conclusion: "action_required" })).toBe(false);
    });
});

describe("checksSummary", () => {
    const done = (conclusion: string) => ({ status: "completed", conclusion });

    it("only says all passed when every check succeeded, counting skipped and neutral as passing", () => {
        expect(checksSummary([done("success"), done("skipped"), done("neutral")])).toBe("all passed");
    });

    it("names what did not pass instead of calling it a pass", () => {
        expect(checksSummary([done("success"), done("cancelled")])).toBe("1 cancelled, 1 passed");
        expect(checksSummary([done("failure"), { status: "in_progress", conclusion: null }, { status: "waiting", conclusion: null }])).toBe(
            "1 failing, 1 running, 1 waiting",
        );
    });
});

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

describe("formatDuration", () => {
    it("writes a span in the units a build log uses", () => {
        expect(formatDuration(4_000)).toBe("4s");
        expect(formatDuration(72_000)).toBe("1m 12s");
        expect(formatDuration(3_780_000)).toBe("1h 3m");
        expect(formatDuration(null)).toBe("—");
    });
});

describe("formatAgo", () => {
    it("picks the one unit that fits", () => {
        expect(formatAgo("2026-01-01T11:59:30Z", NOW)).toBe("30s ago");
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

describe("failedStep", () => {
    it("finds the step a job stopped at", () => {
        const failing = job("completed", "failure", [
            step("checkout", "completed", "success"),
            step("test", "completed", "failure"),
            step("upload", "completed", "skipped"),
        ]);
        expect(failedStep(failing)?.name).toBe("test");
    });

    it("finds nothing in a job that passed", () => {
        expect(failedStep(job("completed", "success", [step("checkout", "completed", "success")]))).toBeUndefined();
    });
});

describe("jobsSummary", () => {
    it("counts what has landed and what broke", () => {
        expect(jobsSummary([job("completed", "success"), job("completed", "failure"), job("in_progress", null)])).toEqual({
            done: 2,
            total: 3,
            failed: 1,
        });
    });

    it("has nothing to count before any job exists", () => {
        expect(jobsSummary([])).toEqual({ done: 0, total: 0, failed: 0 });
    });
});

describe("watchIsNewer", () => {
    const at = (attempt: number, minute: number) => ({ attempt, updatedAt: `2026-09-28T10:${String(minute).padStart(2, "0")}:00Z` });

    it("lets a re-run replace the finished run a watch last saw", () => {
        expect(watchIsNewer(at(1, 30), at(2, 31))).toBe(false);
        expect(watchIsNewer(at(2, 31), at(1, 30))).toBe(true);
    });

    it("lets a refresh of the same attempt win unless the watch saw something later", () => {
        expect(watchIsNewer(at(1, 30), at(1, 30))).toBe(false);
        expect(watchIsNewer(at(1, 32), at(1, 30))).toBe(true);
    });

    it("uses whichever there is", () => {
        expect(watchIsNewer(null, at(1, 30))).toBe(false);
        expect(watchIsNewer(at(1, 30), null)).toBe(true);
    });
});

describe("statusParam", () => {
    it("asks for no status when every run is wanted", () => {
        expect(statusParam("all")).toBeUndefined();
        expect(statusParam("failure")).toBe("failure");
    });
});

describe("eventLabel", () => {
    it("names the events people start runs with", () => {
        expect(eventLabel("pull_request_target")).toBe("Pull request");
        expect(eventLabel("merge_group")).toBe("Merge queue");
    });

    it("spells out an event it has no name for", () => {
        expect(eventLabel("repository_dispatch")).toBe("Repository dispatch");
    });
});
