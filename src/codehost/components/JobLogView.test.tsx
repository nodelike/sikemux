import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Job, JobLog, LogLine } from "../api";

const { jobLog, jumps, copyText } = vi.hoisted(() => ({
    jobLog: vi.fn(),
    jumps: [] as ({ index: number } | null | undefined)[],
    copyText: vi.fn(() => Promise.resolve()),
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
vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), copyText }));

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
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
    copyText.mockClear();
    useToasts.setState({ toasts: [] });
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

describe("what a job's log says when there is nothing to read", () => {
    it("says why the log could not be read, and reads it again on request", async () => {
        jobLog.mockRejectedValueOnce("Not Found").mockResolvedValue(log([line(1, "hello", 0)]));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        expect(screen.getByText("Could not read the log")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await act(async () => {});
        expect(screen.getByText("hello")).toBeTruthy();
    });

    it("says an old log has aged out", async () => {
        jobLog.mockResolvedValue({ lines: [], expired: true, truncated: false });
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        expect(screen.getByText("The log is gone")).toBeTruthy();
    });

    it("tells a job that has not written anything yet from one that wrote nothing", async () => {
        jobLog.mockResolvedValue(log([]));
        const view = render(<JobLogView repo={repo} job={makeJob("in_progress")} active step={null} />, { wrapper });
        await act(async () => {});
        expect(screen.getByText("This job has not written anything yet.")).toBeTruthy();
        view.rerender(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />);
        await act(async () => {});
        expect(screen.getByText("This job wrote no log.")).toBeTruthy();
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

    it("counts a single line in the singular, and says a running job is not done", async () => {
        jobLog.mockResolvedValue(log([line(1, "hello", 0)]));
        render(<JobLogView repo={repo} job={makeJob("in_progress")} active step={null} />, { wrapper });
        await act(async () => {});
        expect(document.querySelector(".gha-log-head > .gha-dim")?.textContent).toBe("1 line so far");
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

    it("says when nothing matches, and goes nowhere on Enter", async () => {
        jobLog.mockResolvedValue(log(lines));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        const search = screen.getByPlaceholderText("Search the log");
        fireEvent.change(search, { target: { value: "panic" } });
        fireEvent.keyDown(search, { key: "Enter" });
        expect(screen.getByText("no matches")).toBeTruthy();
        expect(lastJump()).toBeUndefined();
        expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    });

    it("offers no stepping between matches when there is only one", async () => {
        jobLog.mockResolvedValue(log(lines));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        fireEvent.change(screen.getByPlaceholderText("Search the log"), { target: { value: "two" } });
        expect(screen.getByText("1 of 1")).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    });

    it("marks the line a picked step starts at", async () => {
        jobLog.mockResolvedValue(log([line(1, "setting up", 0), line(2, "npm test", 2)]));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={{ number: 2 }} />, { wrapper });
        await act(async () => {});
        expect(screen.getAllByTestId("log-line").map((row) => row.classList.contains("anchor"))).toEqual([false, true]);
    });

    it("ignores a picked step that wrote no lines", async () => {
        jobLog.mockResolvedValue(log([line(1, "setting up", 0)]));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={{ number: 2 }} />, { wrapper });
        await act(async () => {});
        expect(lastJump()).toBeUndefined();
    });

    it("copies the whole log without the runner's markers", async () => {
        jobLog.mockResolvedValue(log([line(1, "##[group]Run npm test", 0), line(2, "ok", 1)]));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        fireEvent.click(screen.getByRole("button", { name: "Copy" }));
        await act(async () => {});
        expect(copyText).toHaveBeenCalledWith("Run npm test\nok");
        expect(useToasts.getState().toasts.map((toast) => toast.text)).toContain("Copied 2 lines");
    });

    it("reads the log again when Refresh is pressed", async () => {
        jobLog.mockResolvedValue(log([line(1, "ok", 0)]));
        render(<JobLogView repo={repo} job={makeJob("completed")} active step={null} />, { wrapper });
        await act(async () => {});
        fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
        await act(async () => {});
        expect(jobLog).toHaveBeenCalledTimes(2);
    });
});
