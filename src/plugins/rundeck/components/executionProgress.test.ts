import { describe, expect, it } from "vitest";
import type { RundeckExecution, RundeckStep, RundeckWorkflowState } from "../api";
import { executionProgress, newestExecutions } from "./executionProgress";

function step(executionState: string): RundeckStep {
    return { id: null, stepctx: null, executionState, startTime: null, endTime: null, nodeStep: null };
}

function state(steps: RundeckStep[], stepCount = steps.length): RundeckWorkflowState {
    return { executionState: "RUNNING", steps, stepCount, completed: false };
}

function execution(id: number, unixtime: number | null, date: string | null = null): RundeckExecution {
    return {
        id,
        status: "succeeded",
        customStatus: null,
        user: null,
        project: null,
        "date-started": { date, unixtime },
        "date-ended": null,
        permalink: null,
        job: null,
        argstring: null,
    };
}

describe("executionProgress", () => {
    it("counts finished workflow steps", () => {
        expect(executionProgress(state([step("SUCCEEDED"), step("RUNNING"), step("NOT_STARTED"), step("NOT_ELIGIBLE")]))).toEqual({
            completed: 2,
            total: 4,
            percent: 50,
        });
    });

    it("uses the reported step count when steps have not all arrived", () => {
        expect(executionProgress(state([step("SUCCEEDED")], 5))).toEqual({ completed: 1, total: 5, percent: 20 });
    });
});

describe("newestExecutions", () => {
    it("sorts by start time without mutating the API array", () => {
        const original = [execution(1, 1_000), execution(3, 3_000), execution(2, 2_000)];

        expect(newestExecutions(original).map((item) => item.id)).toEqual([3, 2, 1]);
        expect(original.map((item) => item.id)).toEqual([1, 3, 2]);
    });

    it("falls back to parsed dates and execution ids", () => {
        const original = [execution(4, null), execution(6, null, "2026-08-31T12:00:00Z"), execution(5, null)];

        expect(newestExecutions(original).map((item) => item.id)).toEqual([6, 5, 4]);
    });
});
