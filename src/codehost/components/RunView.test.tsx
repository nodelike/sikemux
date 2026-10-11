import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Job, Run, RunTick } from "../api";

const api = vi.hoisted(() => ({
    run: vi.fn(),
    runAttempt: vi.fn(),
    pendingApprovals: vi.fn(() => Promise.resolve([])),
    runTiming: vi.fn(),
    artifacts: vi.fn(),
    watchStart: vi.fn(),
    watchStop: vi.fn(() => Promise.resolve()),
    rerun: vi.fn(),
    rerunJob: vi.fn(),
    cancel: vi.fn(() => Promise.resolve()),
    jobLog: vi.fn(() => Promise.resolve({ lines: [], expired: false, truncated: false })),
    annotations: vi.fn(),
    jobSummary: vi.fn(),
    workflowFile: vi.fn(),
    deleteRun: vi.fn(),
    deleteRunLogs: vi.fn(),
}));

import { invalidate } from "../../plugin-api/resources";
import { acceptDialog, dismissDialog, useDialogs } from "../../state/dialog";
import { useToasts } from "../../state/toast";
import { resetView, updateView, useHostView, viewOf } from "../state";
import { RunMenu } from "./RunMenu";
import { RunView } from "./RunView";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";

const host = registerTestHost(api);
const wrapper = ({ children }: { children: React.ReactNode }) => <InHost host={host}>{children}</InHost>;

const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const makeRun = (overrides: Partial<Run> = {}): Run => ({
    id: "7",
    name: "CI",
    title: "Ship it",
    workflowId: "1",
    path: null,
    runNumber: 12,
    attempt: 1,
    event: "push",
    status: "completed",
    conclusion: "failure",
    branch: "main",
    sha: "abc123",
    shortSha: "abc123",
    actor: null,
    avatarUrl: null,
    createdAt: "2026-01-01T12:00:00Z",
    startedAt: "2026-01-01T12:00:00Z",
    updatedAt: "2026-01-01T12:05:00Z",
    pullRequests: [],
    url: "https://github.com/nodelike/sikemux/actions/runs/7",
    ...overrides,
});

const waiting = makeRun({ status: "waiting", conclusion: null });

function setHidden(hidden: boolean) {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    document.dispatchEvent(new Event("visibilitychange"));
}

const makeJob = (overrides: Partial<Job> = {}): Job => ({
    id: "3",
    name: "build",
    status: "completed",
    conclusion: "success",
    startedAt: "2026-01-01T12:00:00Z",
    completedAt: "2026-01-01T12:01:05Z",
    runner: null,
    url: null,
    checkRunId: null,
    steps: [],
    ...overrides,
});

const toasts = () => useToasts.getState().toasts.map((toast) => toast.text);

function answerDialog(yes: boolean) {
    const dialog = useDialogs.getState().dialog;
    if (!dialog) throw new Error("no dialog is open");
    act(() => (yes ? acceptDialog(dialog.id) : dismissDialog(dialog.id)));
}

beforeEach(() => {
    invalidate(() => true);
    resetView("pane");
    useToasts.setState({ toasts: [] });
    for (const mock of Object.values(api)) mock.mockClear();
    api.runTiming.mockReset().mockResolvedValue({ runDurationMs: null, billable: [] });
    api.artifacts.mockReset().mockResolvedValue([]);
    api.rerun.mockReset().mockResolvedValue(undefined);
    api.rerunJob.mockReset().mockResolvedValue(undefined);
    api.annotations.mockReset().mockResolvedValue([]);
    api.jobSummary.mockReset().mockResolvedValue(null);
    api.workflowFile.mockReset().mockResolvedValue({ path: ".github/workflows/ci.yml", text: "on: push" });
    api.deleteRun.mockReset().mockResolvedValue(undefined);
    api.deleteRunLogs.mockReset().mockResolvedValue(undefined);
    api.watchStart.mockReset().mockResolvedValue(1);
    api.runAttempt
        .mockReset()
        .mockImplementation((_repo, runId: string, attempt: number) => Promise.resolve({ run: makeRun({ id: runId, attempt }), jobs: [] }));
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
});

async function renderRun(run: Run, runId = run.id) {
    api.run.mockReset().mockImplementation((_repo, id: string) => Promise.resolve({ run: { ...run, id }, jobs: [] }));
    const view = render(<RunView paneId="pane" repo={repo} runId={runId} openJob={null} active canWrite />, { wrapper });
    await act(async () => {});
    return view;
}

describe("a run waiting for approval", () => {
    it("offers to cancel it and nothing that only a finished run allows", async () => {
        await renderRun(waiting);
        expect(screen.getByRole("button", { name: "Cancel run" })).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Re-run all jobs" })).toBeNull();
        expect(screen.queryByRole("button", { name: "Re-run failed jobs" })).toBeNull();
    });

    it("keeps its menu to the actions that do not need it to be over", () => {
        render(<RunMenu run={waiting} repo={repo} canWrite onDeleted={() => {}} />, { wrapper });
        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        expect(screen.getByRole("menuitem", { name: "Copy link" })).toBeTruthy();
        expect(screen.queryByRole("menuitem", { name: "Re-run all with debug logs" })).toBeNull();
        expect(screen.queryByRole("menuitem", { name: "Delete all logs" })).toBeNull();
        expect(screen.queryByRole("menuitem", { name: "Delete run" })).toBeNull();
    });

    it("is watched live", async () => {
        await renderRun(waiting);
        expect(api.watchStart).toHaveBeenCalledTimes(1);
    });
});

describe("RunView", () => {
    it("re-runs once however many times the button is pressed", async () => {
        await renderRun(makeRun());
        const button = screen.getByRole("button", { name: "Re-run all jobs" });
        fireEvent.click(button);
        fireEvent.click(button);
        await act(async () => {});
        expect(api.rerun).toHaveBeenCalledTimes(1);
    });

    it("asks a newly opened run for its latest attempt, not the attempt picked on the one before", async () => {
        const view = await renderRun(makeRun({ attempt: 2 }));
        fireEvent.click(screen.getByRole("button", { name: "#1" }));
        await act(async () => {});
        expect(api.runAttempt).toHaveBeenCalledWith(repo, "7", 1);
        api.runAttempt.mockClear();

        view.rerender(<RunView paneId="pane" repo={repo} runId="8" openJob={null} active canWrite />);
        await act(async () => {});
        expect(api.runAttempt).not.toHaveBeenCalled();
        expect(api.run).toHaveBeenCalledWith(repo, "8");
    });

    it("starts the watch again, and says why, when it gives up on a run still going", async () => {
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        await renderRun(makeRun({ status: "in_progress", conclusion: null }));
        const onTick = api.watchStart.mock.calls[0][2] as (tick: RunTick) => void;
        act(() => onTick({ run: null, jobs: [], error: "GitHub is rate limiting this token", finished: true, fatal: false, signedOut: false }));
        expect(screen.getByText("GitHub is rate limiting this token")).toBeTruthy();
        await act(async () => {
            vi.advanceTimersByTime(60_000);
        });
        expect(api.watchStart).toHaveBeenCalledTimes(2);
    });

    it("stops for good, still saying why, when watching again cannot help", async () => {
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        await renderRun(makeRun({ status: "in_progress", conclusion: null }));
        const onTick = api.watchStart.mock.calls[0][2] as (tick: RunTick) => void;
        act(() => onTick({ run: null, jobs: [], error: "github: not signed in", finished: true, fatal: true, signedOut: true }));
        expect(screen.getByText("github: not signed in")).toBeTruthy();
        await act(async () => {
            vi.advanceTimersByTime(10 * 60_000);
        });
        await act(async () => setHidden(true));
        await act(async () => setHidden(false));
        expect(api.watchStart).toHaveBeenCalledTimes(1);
    });

    it("stops watching while the window is hidden and picks up again when it is shown", async () => {
        await renderRun(makeRun({ status: "in_progress", conclusion: null }));
        expect(api.watchStart).toHaveBeenCalledTimes(1);
        await act(async () => setHidden(true));
        expect(api.watchStop).toHaveBeenCalledWith(1);
        await act(async () => setHidden(false));
        expect(api.watchStart).toHaveBeenCalledTimes(2);
    });
});

function Pane({ canWrite = true }: { canWrite?: boolean }) {
    const view = useHostView("pane");
    return <RunView paneId="pane" repo={repo} runId="7" openJob={view.job} active canWrite={canWrite} />;
}

async function renderPane(run: Run, jobs: Job[] = [], canWrite = true) {
    api.run.mockReset().mockResolvedValue({ run, jobs });
    const view = render(<Pane canWrite={canWrite} />, { wrapper });
    await act(async () => {});
    return view;
}

const card = () => document.querySelector(".gha-merge-box") as HTMLElement;

describe("the run's card", () => {
    it("cancels a run only once the person confirms", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null }));
        fireEvent.click(screen.getByRole("button", { name: "Cancel run" }));
        await act(async () => {});
        answerDialog(false);
        await act(async () => {});
        expect(api.cancel).not.toHaveBeenCalled();

        fireEvent.click(screen.getByRole("button", { name: "Cancel run" }));
        await act(async () => {});
        answerDialog(true);
        await act(async () => {});
        expect(api.cancel).toHaveBeenCalledWith(repo, "7");
        expect(toasts()).toContain("Cancelled the run");
    });

    it("offers nothing that changes the run to someone who cannot write", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null }), [], false);
        expect(screen.queryByRole("button", { name: "Cancel run" })).toBeNull();
        cleanup();
        await renderPane(makeRun(), [], false);
        expect(screen.queryByRole("button", { name: "Re-run all jobs" })).toBeNull();
        expect(screen.queryByRole("button", { name: "Re-run failed jobs" })).toBeNull();
    });
});

describe("a run opened from a failing check", () => {
    it("opens the log of the job that failed", async () => {
        updateView("pane", { pickFailed: true });
        await renderPane(makeRun(), [makeJob({ id: "1", name: "lint" }), makeJob({ id: "2", name: "test", conclusion: "failure" })]);
        expect(viewOf("pane")).toMatchObject({ pickFailed: false, job: "2", runTab: "logs" });
        expect(screen.getByRole("tab", { name: "Logs · test" }).getAttribute("aria-selected")).toBe("true");
    });

    it("stays on the summary when no job failed", async () => {
        updateView("pane", { pickFailed: true });
        await renderPane(makeRun({ conclusion: "success" }), [makeJob()]);
        expect(viewOf("pane")).toMatchObject({ pickFailed: false, job: null, runTab: "summary" });
    });

    it("waits for the jobs before picking one", async () => {
        updateView("pane", { pickFailed: true });
        await renderPane(makeRun(), []);
        expect(viewOf("pane").pickFailed).toBe(true);
    });
});

describe("a job's log", () => {
    async function openJob(job: Job, canWrite = true) {
        updateView("pane", { job: job.id, runTab: "logs" });
        await renderPane(makeRun(), [job], canWrite);
    }

    it("re-runs the job, with or without the runner's debug logging", async () => {
        await openJob(makeJob());
        fireEvent.click(screen.getByRole("button", { name: "Re-run this job" }));
        await act(async () => {});
        expect(api.rerunJob).toHaveBeenLastCalledWith(repo, "3", false);
        expect(toasts()).toContain("Re-running build");
        fireEvent.click(screen.getByRole("button", { name: "with debug logs" }));
        await act(async () => {});
        expect(api.rerunJob).toHaveBeenLastCalledWith(repo, "3", true);
    });

    it("offers no re-run of a job still going, or to someone who cannot write", async () => {
        await openJob(makeJob({ status: "in_progress", conclusion: null, completedAt: null }));
        expect(screen.queryByRole("button", { name: "Re-run this job" })).toBeNull();
        cleanup();
        await openJob(makeJob(), false);
        expect(screen.queryByRole("button", { name: "Re-run this job" })).toBeNull();
        expect(screen.queryByRole("button", { name: "with debug logs" })).toBeNull();
    });
});

describe("the run's summary", () => {
    it("shows the workflow file on request, and hides it again", async () => {
        await renderPane(makeRun({ path: ".github/workflows/ci.yml" }));
        fireEvent.click(screen.getByRole("button", { name: "Workflow file" }));
        await act(async () => {});
        expect(api.workflowFile).toHaveBeenCalledWith(repo, "1");
        expect(screen.getByText(".github/workflows/ci.yml")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Hide workflow file" }));
        expect(screen.queryByText(".github/workflows/ci.yml")).toBeNull();
    });

    it("offers no workflow file for a run with no file behind it", async () => {
        await renderPane(makeRun({ path: "dynamic/pages/pages-build-deployment" }));
        expect(screen.queryByRole("button", { name: "Workflow file" })).toBeNull();
    });
});

describe("watching a run", () => {
    const going = () => makeRun({ status: "in_progress", conclusion: null, updatedAt: "2026-01-01T12:01:00Z" });

    it("shows what the watch sees once it is newer than the last read, and keeps the jobs it read until the watch has some", async () => {
        await renderPane(going(), [makeJob({ status: "in_progress", conclusion: null, completedAt: null })]);
        const onTick = api.watchStart.mock.calls[0][2] as (tick: RunTick) => void;
        const newer = makeRun({ status: "in_progress", conclusion: null, updatedAt: "2026-01-01T12:02:00Z", title: "Ship it now" });
        act(() => onTick({ run: newer, jobs: [], error: null, finished: false, fatal: false, signedOut: false }));
        expect(screen.getByRole("heading").textContent).toBe("Ship it now #12");
        expect(within(document.querySelector(".pr-files") as HTMLElement).getByText("build")).toBeTruthy();

        act(() => onTick({ run: newer, jobs: [makeJob({ id: "9", name: "deploy" })], error: null, finished: false, fatal: false, signedOut: false }));
        expect(within(document.querySelector(".pr-files") as HTMLElement).getByText("deploy")).toBeTruthy();
    });

    it("stops once the run it watches has finished, and reads the list again", async () => {
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        await renderPane(going());
        const onTick = api.watchStart.mock.calls[0][2] as (tick: RunTick) => void;
        api.run.mockClear();
        const done = makeRun({ updatedAt: "2026-01-01T12:09:00Z", conclusion: "success" });
        act(() => onTick({ run: done, jobs: [], error: null, finished: true, fatal: false, signedOut: false }));
        await act(async () => {
            vi.advanceTimersByTime(10 * 60_000);
        });
        expect(api.watchStart).toHaveBeenCalledTimes(1);
        expect(api.run).toHaveBeenCalled();
        expect(within(card()).getByText("Passed")).toBeTruthy();
    });

    it("stops a watch that only started after the run was closed", async () => {
        let started: (id: number) => void = () => {};
        api.watchStart.mockReturnValue(new Promise((resolve) => (started = resolve)));
        const view = await renderPane(going());
        view.unmount();
        await act(async () => started(5));
        expect(api.watchStop).toHaveBeenCalledWith(5);
    });

    it("stops a watch that started after the window was hidden, and starts one once it is shown", async () => {
        let started: (id: number) => void = () => {};
        api.watchStart.mockReturnValueOnce(new Promise((resolve) => (started = resolve))).mockResolvedValue(6);
        await renderPane(going());
        await act(async () => setHidden(true));
        await act(async () => started(5));
        expect(api.watchStop).toHaveBeenCalledWith(5);
        expect(api.watchStart).toHaveBeenCalledTimes(1);
        await act(async () => setHidden(false));
        expect(api.watchStart).toHaveBeenCalledTimes(2);
    });

    it("ignores ticks from a watch that was stopped", async () => {
        await renderPane(going());
        const onTick = api.watchStart.mock.calls[0][2] as (tick: RunTick) => void;
        await act(async () => setHidden(true));
        act(() => onTick({ run: null, jobs: [], error: "stale", finished: false, fatal: false, signedOut: false }));
        expect(screen.queryByText("stale")).toBeNull();
    });
});

describe("the run's menu", () => {
    async function openMenu(run: Run = makeRun(), canWrite = true) {
        const onDeleted = vi.fn();
        render(<RunMenu run={run} repo={repo} canWrite={canWrite} onDeleted={onDeleted} />, { wrapper });
        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        return onDeleted;
    }

    it("deletes the logs only once confirmed", async () => {
        await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Delete all logs" }));
        answerDialog(false);
        await act(async () => {});
        expect(api.deleteRunLogs).not.toHaveBeenCalled();

        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Delete all logs" }));
        answerDialog(true);
        await act(async () => {});
        expect(api.deleteRunLogs).toHaveBeenCalledWith(repo, "7");
        expect(toasts()).toContain("Deleted the logs");
    });

    it("deletes the run once confirmed and says so to whoever showed it", async () => {
        const onDeleted = await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Delete run" }));
        answerDialog(false);
        await act(async () => {});
        expect(api.deleteRun).not.toHaveBeenCalled();

        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Delete run" }));
        answerDialog(true);
        await act(async () => {});
        expect(api.deleteRun).toHaveBeenCalledWith(repo, "7");
        expect(onDeleted).toHaveBeenCalledTimes(1);
        expect(toasts()).toContain("Deleted run #12");
    });

    it("keeps the run when deleting it fails", async () => {
        api.deleteRun.mockRejectedValue(new Error("locked"));
        api.deleteRunLogs.mockRejectedValue(new Error("locked"));
        const onDeleted = await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Delete run" }));
        answerDialog(true);
        await act(async () => {});
        expect(onDeleted).not.toHaveBeenCalled();
        expect(toasts()).toContain("Could not delete the run: locked");

        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Delete all logs" }));
        answerDialog(true);
        await act(async () => {});
        expect(toasts()).toContain("Could not delete the logs: locked");
    });

    it("offers only reading actions to someone who cannot write", async () => {
        await openMenu(makeRun(), false);
        expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Copy link", "Open on Test host"]);
    });
});
