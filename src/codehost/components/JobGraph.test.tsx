import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Job, Run } from "../api";
import { JobGraph } from "./JobGraph";

const run = (overrides: Partial<Run> = {}): Run => ({
    id: "7",
    name: "CI",
    title: "Ship it",
    workflowId: "1",
    path: ".github/workflows/ci.yml",
    runNumber: 12,
    attempt: 1,
    event: "push",
    status: "completed",
    conclusion: "success",
    branch: "main",
    sha: "abc",
    shortSha: "abc",
    actor: null,
    avatarUrl: null,
    createdAt: "2026-01-01T12:00:00Z",
    startedAt: "2026-01-01T12:00:00Z",
    updatedAt: "2026-01-01T12:05:00Z",
    pullRequests: [],
    url: "",
    ...overrides,
});

const job = (overrides: Partial<Job> = {}): Job => ({
    id: "1",
    name: "build",
    status: "completed",
    conclusion: "success",
    startedAt: "2026-01-01T12:00:00Z",
    completedAt: "2026-01-01T12:00:42Z",
    runner: null,
    url: null,
    checkRunId: null,
    steps: [],
    ...overrides,
});

const now = Date.parse("2026-01-01T12:02:00Z");

function graph(props: Partial<Parameters<typeof JobGraph>[0]> = {}) {
    const onOpen = vi.fn();
    const onToggleFile = vi.fn();
    render(<JobGraph run={run()} jobs={[job()]} now={now} openJob={null} onOpen={onOpen} fileShown={false} onToggleFile={onToggleFile} {...props} />);
    return { onOpen, onToggleFile };
}

afterEach(cleanup);

describe("JobGraph", () => {
    it("is headed by the workflow's file name, or the workflow's name when it has no file", () => {
        graph();
        expect(screen.getByText("ci.yml")).toBeTruthy();
        cleanup();
        graph({ run: run({ path: null }) });
        expect(screen.getByText("CI")).toBeTruthy();
    });

    it("counts the jobs done and the jobs that failed", () => {
        graph({
            jobs: [
                job({ id: "1" }),
                job({ id: "2", name: "test", conclusion: "failure" }),
                job({ id: "3", name: "deploy", status: "in_progress", conclusion: null, completedAt: null }),
            ],
        });
        expect(document.querySelector(".gha-graph-head .gha-dim")?.textContent).toBe("2 of 3 jobs done · 1 failed");
    });

    it("counts a single job in the singular and says nothing of failures when none failed", () => {
        graph();
        expect(document.querySelector(".gha-graph-head .gha-dim")?.textContent).toBe("1 of 1 job done");
    });

    it("says there are no jobs yet", () => {
        graph({ jobs: [] });
        expect(screen.getByText("No jobs yet")).toBeTruthy();
        expect(document.querySelector(".gha-graph-head .gha-dim")).toBeNull();
    });

    it("toggles the workflow file, when the host can show one and the run came from one", () => {
        const { onToggleFile } = graph();
        fireEvent.click(screen.getByRole("button", { name: "Workflow file" }));
        expect(onToggleFile).toHaveBeenCalledTimes(1);
        cleanup();
        graph({ fileShown: true });
        expect(screen.getByRole("button", { name: "Hide workflow file" })).toBeTruthy();
        cleanup();
        graph({ onToggleFile: null });
        expect(screen.queryByRole("button", { name: "Workflow file" })).toBeNull();
        cleanup();
        graph({ run: run({ path: "dynamic/pages/pages-build-deployment" }) });
        expect(screen.queryByRole("button", { name: "Workflow file" })).toBeNull();
    });

    it("boxes a called workflow's jobs under its name and opens the one clicked", () => {
        const { onOpen } = graph({
            jobs: [job({ id: "1", name: "Checks / Lint" }), job({ id: "2", name: "Checks / Test" })],
            openJob: "2",
        });
        expect(document.querySelector(".gha-graph-group-title")?.textContent).toBe("Checks");
        const test = screen.getByTitle("Checks / Test");
        expect(test.dataset.on).toBe("1");
        expect(screen.getByTitle("Checks / Lint").dataset.on).toBe("0");
        expect(screen.getByText("Test")).toBeTruthy();
        fireEvent.click(test);
        expect(onOpen).toHaveBeenCalledWith("2");
    });

    it("times a finished job to its end, a running one to now, and leaves skipped and unstarted jobs untimed", () => {
        graph({
            jobs: [
                job({ id: "1", name: "done" }),
                job({ id: "2", name: "going", status: "in_progress", conclusion: null, completedAt: null }),
                job({ id: "3", name: "skipped", conclusion: "skipped" }),
                job({ id: "4", name: "waiting", status: "queued", conclusion: null, startedAt: null, completedAt: null }),
            ],
        });
        const took = (name: string) => screen.getByTitle(name).querySelector(".gha-graph-job-took")?.textContent ?? null;
        expect(took("done")).toBe("42s");
        expect(took("going")).toBe("2m 0s");
        expect(took("skipped")).toBeNull();
        expect(took("waiting")).toBeNull();
    });
});
