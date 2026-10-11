import { describe, expect, it } from "vitest";
import type { RundeckExecution } from "./api";
import { runNotice } from "./runNotice";

const BRANCH_OPTIONS = ["branch"];

function execution(status: string, change: Partial<RundeckExecution> = {}): RundeckExecution {
    return {
        id: 4812,
        status,
        customStatus: null,
        user: "ankit",
        project: "shop",
        "date-started": { date: "2026-10-08T10:00:00Z", unixtime: null },
        "date-ended": { date: "2026-10-08T10:04:12Z", unixtime: null },
        permalink: null,
        job: { id: "j1", name: "api", group: "staging/backend", project: "shop", options: { branch: "feature/cart" } },
        argstring: null,
        ...change,
    };
}

describe("runNotice", () => {
    it("says which branch went where, and how long it took", () => {
        expect(runNotice(execution("succeeded"), BRANCH_OPTIONS)).toEqual({
            ok: true,
            title: "Deployed feature/cart to staging/backend",
            body: "api · took 4m 12s",
        });
    });

    it("says a deploy failed, was aborted or timed out", () => {
        expect(runNotice(execution("failed"), BRANCH_OPTIONS)).toEqual({
            ok: false,
            title: "Deploy of feature/cart to staging/backend failed",
            body: "api · after 4m 12s",
        });
        expect(runNotice(execution("aborted"), BRANCH_OPTIONS).title).toBe("Deploy of feature/cart to staging/backend was aborted");
        expect(runNotice(execution("timedout"), BRANCH_OPTIONS).title).toBe("Deploy of feature/cart to staging/backend timed out");
        expect(runNotice(execution("other", { customStatus: "rolled back" }), BRANCH_OPTIONS).title).toBe(
            "Deploy of feature/cart to staging/backend ended: rolled back",
        );
    });

    it("names a job that takes no branch by itself", () => {
        const job = { id: "j2", name: "rotate-logs", group: null, project: "ops", options: {} };
        expect(runNotice(execution("succeeded", { job }), BRANCH_OPTIONS)).toEqual({ ok: true, title: "rotate-logs finished", body: "took 4m 12s" });
        expect(runNotice(execution("failed", { job }), BRANCH_OPTIONS).title).toBe("rotate-logs on ops failed");
    });

    it("falls back to the execution number when Rundeck says little", () => {
        const bare = execution("failed", { job: null, project: null, "date-started": null, "date-ended": null });
        expect(runNotice(bare, BRANCH_OPTIONS)).toEqual({ ok: false, title: "Execution #4812 failed", body: "#4812" });
    });
});
