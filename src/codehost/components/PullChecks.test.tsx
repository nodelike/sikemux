import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Run } from "../api";

const api = vi.hoisted(() => ({ runs: vi.fn() }));

import { invalidate } from "../../plugin-api/resources";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { PullChecks } from "./PullChecks";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const makeRun = (overrides: Partial<Run> = {}): Run => ({
    id: "7",
    name: "CI",
    title: "Ship it",
    workflowId: "1",
    path: null,
    runNumber: 12,
    attempt: 1,
    event: "pull_request",
    status: "completed",
    conclusion: "success",
    branch: "feat/x",
    sha: "abc",
    shortSha: "abc",
    actor: null,
    avatarUrl: null,
    createdAt: "2026-01-01T12:00:00Z",
    startedAt: "2026-01-01T12:00:00Z",
    updatedAt: "2026-01-01T12:01:30Z",
    pullRequests: [31],
    url: "",
    ...overrides,
});

async function renderChecks(runs: Run[], onOpenRun = vi.fn()) {
    api.runs.mockResolvedValue({ runs, total: runs.length, nextPage: null });
    const view = render(
        <InHost host={host}>
            <PullChecks repo={repo} sha="abc" active onOpenRun={onOpenRun} />
        </InHost>,
    );
    await act(async () => {});
    return { view, onOpenRun };
}

const headline = () => document.querySelector(".gha-merge-title")?.textContent;
const overall = () => document.querySelector(".gha-merge-row [role=img]")?.getAttribute("aria-label");

beforeEach(() => {
    invalidate(() => true);
    api.runs.mockReset();
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("PullChecks", () => {
    it("asks for the runs on the pull request's head commit", async () => {
        await renderChecks([makeRun()]);
        expect(api.runs).toHaveBeenCalledWith({ ...repo, headSha: "abc", perPage: 30 });
    });

    it("shows nothing when no run touched the commit", async () => {
        const { view } = await renderChecks([]);
        expect(view.container.textContent).toBe("");
    });

    it("says all passed when every run passed or was skipped", async () => {
        await renderChecks([makeRun({ id: "1" }), makeRun({ id: "2", conclusion: "skipped" })]);
        expect(headline()).toBe("All checks passed");
        expect(overall()).toBe("Passed");
    });

    it("leads with a failure over anything still going", async () => {
        await renderChecks([
            makeRun({ id: "1", conclusion: "failure" }),
            makeRun({ id: "2", status: "in_progress", conclusion: null }),
            makeRun({ id: "3" }),
        ]);
        expect(headline()).toBe("Checks: 1 failing, 1 running, 1 passed");
        expect(overall()).toBe("Failed");
    });

    it("reads as running while a run is queued, before one waiting for approval", async () => {
        await renderChecks([makeRun({ id: "1", status: "queued", conclusion: null }), makeRun({ id: "2", status: "waiting", conclusion: null })]);
        expect(overall()).toBe("Running");
    });

    it("reads as waiting when a run is held for approval and nothing else is going", async () => {
        await renderChecks([makeRun({ id: "1", status: "waiting", conclusion: null }), makeRun({ id: "2" })]);
        expect(headline()).toBe("Checks: 1 waiting, 1 passed");
        expect(overall()).toBe("Waiting for approval");
    });

    it("lists each run with what started it, how it went and how long it took, and opens it on click", async () => {
        const { onOpenRun } = await renderChecks([makeRun({ name: "Lint", runNumber: 4 })]);
        const check = screen.getByRole("button", { name: /Lint/ });
        expect(check.textContent).toBe("Lint #4Pull requestPassed1m 30s");
        fireEvent.click(check);
        expect(onOpenRun).toHaveBeenCalledWith("7");
    });

    it("reads the checks again every ten seconds while one is going, and stops once they are done", async () => {
        vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
        await renderChecks([makeRun({ status: "in_progress", conclusion: null })]);
        expect(api.runs).toHaveBeenCalledTimes(1);
        api.runs.mockResolvedValue({ runs: [makeRun()], total: 1, nextPage: null });
        await act(async () => {
            vi.advanceTimersByTime(10_000);
        });
        expect(api.runs).toHaveBeenCalledTimes(2);
        expect(headline()).toBe("All checks passed");
        await act(async () => {
            vi.advanceTimersByTime(60_000);
        });
        expect(api.runs).toHaveBeenCalledTimes(2);
    });
});
