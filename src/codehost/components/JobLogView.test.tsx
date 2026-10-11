import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Job, JobLog, LogLine } from "../api";

const { jobLog, jumps } = vi.hoisted(() => ({
    jobLog: vi.fn(),
    jumps: [] as ({ index: number } | null | undefined)[],
}));
vi.mock("../../plugin-api/ui", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    VirtualLogList: ({
        jumpTo,
        items,
        rowClassName,
        renderRow,
    }: {
        jumpTo?: { index: number } | null;
        items: LogLine[];
        rowClassName: (line: LogLine, index: number) => string;
        renderRow: (line: LogLine, index: number) => React.ReactNode;
    }) => {
        jumps.push(jumpTo);
        return items.map((item, index) => (
            <div key={item.number} data-testid="log-line" className={rowClassName(item, index)}>
                {renderRow(item, index)}
            </div>
        ));
    },
}));

import { invalidate } from "../../plugin-api/resources";
import { JobLogView } from "./JobLogView";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";

const host = registerTestHost({ jobLog });
const wrapper = ({ children }: { children: React.ReactNode }) => <InHost host={host}>{children}</InHost>;

const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const line = (number: number, text: string, second: number): LogLine => ({
    number,
    text,
    timestamp: `2026-01-01T12:00:${String(second).padStart(2, "0")}.500Z`,
});

const makeJob = (status: string): Job => ({
    id: "3",
    name: "build",
    status,
    conclusion: status === "completed" ? "failure" : null,
    startedAt: "2026-01-01T12:00:00Z",
    completedAt: null,
    runner: null,
    url: null,
    checkRunId: null,
    steps: [
        { number: 1, name: "Set up", status: "completed", conclusion: "success", startedAt: "2026-01-01T12:00:00Z", completedAt: null },
        { number: 2, name: "Test", status: "in_progress", conclusion: null, startedAt: "2026-01-01T12:00:02Z", completedAt: null },
    ],
});

const log = (lines: LogLine[]): JobLog => ({ lines, expired: false, truncated: false });

const lastJump = () => jumps.filter(Boolean).at(-1);

beforeEach(() => {
    invalidate(() => true);
    jumps.length = 0;
    jobLog.mockReset();
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("JobLogView", () => {
    it("jumps to a picked step once, and stays put as new lines arrive", async () => {
        vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
        const first = [line(1, "setting up", 0), line(2, "npm test", 2), line(3, "ok 1", 3)];
        jobLog.mockResolvedValueOnce(log(first)).mockResolvedValue(log([...first, line(4, "ok 2", 4)]));
        render(<JobLogView repo={repo} job={makeJob("in_progress")} active step={{ number: 2 }} />, { wrapper });
        await act(async () => {});
        const jumped = lastJump();
        expect(jumped).toEqual({ index: 1 });

        await act(async () => {
            vi.advanceTimersByTime(5_000);
        });
        expect(jobLog).toHaveBeenCalledTimes(2);
        expect(lastJump()).toBe(jumped);
    });

    it("lands on the first match as soon as something is typed, and Enter moves on from there", async () => {
        jobLog.mockResolvedValue(log([line(1, "start", 0), line(2, "error one", 1), line(3, "fine", 2), line(4, "error two", 3)]));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        const search = screen.getByPlaceholderText("Search the log");

        fireEvent.change(search, { target: { value: "error" } });
        expect(lastJump()).toEqual({ index: 1 });
        expect(screen.getByText("1 of 2")).toBeTruthy();

        fireEvent.keyDown(search, { key: "Enter" });
        expect(lastJump()).toEqual({ index: 3 });
        expect(screen.getByText("2 of 2")).toBeTruthy();
    });

    it("reads the log one last time when the job finishes", async () => {
        jobLog.mockResolvedValueOnce(log([line(1, "running", 0)])).mockResolvedValue(log([line(1, "running", 0), line(2, "Error: boom", 1)]));
        const view = render(<JobLogView repo={repo} job={makeJob("in_progress")} active step={null} />, { wrapper });
        await act(async () => {});
        expect(jobLog).toHaveBeenCalledTimes(1);

        view.rerender(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />);
        await act(async () => {});
        expect(jobLog).toHaveBeenCalledTimes(2);
    });
});

describe("reading a job's log", () => {
    const lines = [line(1, "##[group]Run npm test", 0), line(2, "error one", 1), line(3, "##[endgroup]", 2), line(4, "", 3), line(5, "error two", 4)];

    it("strips the runner's section markers and marks where a section opens", async () => {
        jobLog.mockResolvedValue(log(lines));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        const rows = screen.getAllByTestId("log-line");
        expect(rows.map((row) => row.querySelector(".gha-log-text")?.textContent)).toEqual(["Run npm test", "error one", " ", " ", "error two"]);
        expect(rows.map((row) => row.classList.contains("group"))).toEqual([true, false, false, false, false]);
        expect(screen.getByText("5 lines")).toBeTruthy();
    });

    it("highlights every match and the one in focus, and wraps backwards with Shift+Enter", async () => {
        jobLog.mockResolvedValue(log(lines));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        const search = screen.getByPlaceholderText("Search the log");
        fireEvent.change(search, { target: { value: "ERROR" } });
        const classes = () => screen.getAllByTestId("log-line").map((row) => row.className.replace("gha-log-line", "").trim());
        expect(classes()).toEqual(["group", "hit current", "", "", "hit"]);

        fireEvent.keyDown(search, { key: "Enter", shiftKey: true });
        expect(lastJump()).toEqual({ index: 4 });
        expect(screen.getByText("2 of 2")).toBeTruthy();
        fireEvent.keyDown(search, { key: "Escape" });
        expect(screen.getByText("2 of 2")).toBeTruthy();

        fireEvent.click(screen.getByRole("button", { name: "Next" }));
        expect(screen.getByText("1 of 2")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Previous" }));
        expect(screen.getByText("2 of 2")).toBeTruthy();
    });
});
