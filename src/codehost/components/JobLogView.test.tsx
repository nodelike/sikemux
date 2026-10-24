import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Job, JobLogLine, LogMark, OpenJobLog } from "../api";

import { invalidate } from "../../plugin-api/resources";
import { JobLogView } from "./JobLogView";
import { ciCore, InHost, registerTestHost, TEST_HOST } from "../testHost";

/** The log the core answers with. */
const jobLog = vi.fn<() => OpenJobLog>();
const core = ciCore({}, undefined, { codeHostJobLog: () => jobLog() });
const host = registerTestHost({});
const wrapper = ({ children }: { children: React.ReactNode }) => <InHost host={host}>{children}</InHost>;

const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const line = (number: number, text: string, mark: LogMark | null = null): JobLogLine => ({ number, text, spans: [], mark, script: false });

const makeJob = (status: string): Job => ({
    id: "3",
    name: "build",
    status,
    conclusion: status === "completed" ? "failure" : null,
    outcome: status === "completed" ? "failure" : "running",
    startedAt: "2026-01-01T12:00:00Z",
    completedAt: null,
    runner: null,
    url: null,
    checkRunId: null,
    steps: [
        {
            number: 1,
            name: "Set up",
            status: "completed",
            conclusion: "success",
            outcome: "success",
            startedAt: "2026-01-01T12:00:00Z",
            completedAt: null,
        },
        {
            number: 2,
            name: "Test",
            status: "in_progress",
            conclusion: null,
            outcome: "running",
            startedAt: "2026-01-01T12:00:02Z",
            completedAt: null,
        },
    ],
});

/** The whole log as the step that ran first. */
const log = (lines: JobLogLine[]): OpenJobLog => ({
    lines,
    from: 0,
    total: lines.length,
    sections: [
        { step: 1, from: 0, to: lines.length },
        { step: 2, from: lines.length, to: lines.length },
    ],
    expired: false,
    truncated: false,
});

const view = (job: Job) => <JobLogView project={null} repo={repo} runId="7" job={job} active now={Date.parse("2026-01-01T12:01:00Z")} />;

const rows = () => [...document.querySelectorAll<HTMLElement>(".gha-log-line")];
const current = () =>
    rows()
        .find((row) => row.classList.contains("current"))
        ?.querySelector(".gha-log-text")?.textContent;

beforeEach(() => {
    invalidate(() => true);
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
    jobLog.mockReset();
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
});

describe("JobLogView", () => {
    it("lands on the first match as soon as something is typed, and Enter moves on from there", async () => {
        jobLog.mockReturnValue(log([line(1, "start"), line(2, "error one"), line(3, "fine"), line(4, "error two")]));
        render(view(makeJob("completed")), { wrapper });
        await act(async () => {});
        const search = screen.getByPlaceholderText("Search the log");

        fireEvent.change(search, { target: { value: "error" } });
        expect(current()).toBe("error one");
        expect(screen.getByText("1 of 2")).toBeTruthy();

        fireEvent.keyDown(search, { key: "Enter" });
        expect(current()).toBe("error two");
        expect(screen.getByText("2 of 2")).toBeTruthy();
    });

    it("reads the log again when the core says it moved", async () => {
        jobLog.mockReturnValueOnce(log([line(1, "running")])).mockReturnValue(log([line(1, "running"), line(2, "Error: boom")]));
        render(view(makeJob("in_progress")), { wrapper });
        await act(async () => {});
        expect(jobLog).toHaveBeenCalledTimes(1);

        await act(async () => core.listsMoved(["jobLog"]));
        expect(jobLog).toHaveBeenCalledTimes(2);
        expect(screen.getByText("2 lines so far")).toBeTruthy();
    });
});

describe("reading a job's log", () => {
    const lines = [line(1, "Run npm test", "group"), line(2, "error one"), line(3, "", "endgroup"), line(4, ""), line(5, "error two")];

    it("marks where a section opens", async () => {
        jobLog.mockReturnValue(log(lines));
        render(view(makeJob("completed")), { wrapper });
        await act(async () => {});
        fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
        expect(rows().map((row) => row.querySelector(".gha-log-text")?.textContent)).toEqual(["Run npm test", "error one", " ", " ", "error two"]);
        expect(rows().map((row) => row.classList.contains("group"))).toEqual([true, false, false, false, false]);
        expect(screen.getByText("5 lines")).toBeTruthy();
    });

    it("highlights every match and the one in focus, and wraps backwards with Shift+Enter", async () => {
        jobLog.mockReturnValue(log(lines));
        render(view(makeJob("completed")), { wrapper });
        await act(async () => {});
        const search = screen.getByPlaceholderText("Search the log");
        fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
        fireEvent.change(search, { target: { value: "ERROR" } });
        const classes = () => rows().map((row) => row.className.replace("gha-log-line", "").trim());
        expect(classes()).toEqual(["group", "hit current", "", "", "hit"]);

        fireEvent.keyDown(search, { key: "Enter", shiftKey: true });
        expect(current()).toBe("error two");
        expect(screen.getByText("2 of 2")).toBeTruthy();
        fireEvent.keyDown(search, { key: "Escape" });
        expect(screen.getByText("2 of 2")).toBeTruthy();

        fireEvent.click(screen.getByRole("button", { name: "Next" }));
        expect(screen.getByText("1 of 2")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Previous" }));
        expect(screen.getByText("2 of 2")).toBeTruthy();
    });
});
