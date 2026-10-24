import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Job, OpenRun, Run } from "../api";

const api = vi.hoisted(() => ({
    pendingApprovals: vi.fn(() => Promise.resolve([])),
    artifacts: vi.fn(),
    rerun: vi.fn(),
    rerunJob: vi.fn(),
    cancel: vi.fn(() => Promise.resolve()),
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
import { ciCore, InHost, registerTestHost, TEST_HOST } from "../testHost";

/** The run the core answers with, for what it was asked. */
const runs = vi.fn<(params: Record<string, unknown>) => OpenRun>();
const core = ciCore({}, undefined, {
    codeHostRun: (params) => runs(params),
    codeHostJobLog: () => ({ lines: [], from: 0, total: 0, sections: [], expired: false, truncated: false }),
});
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
    outcome: "failure",
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

const waiting = makeRun({ status: "waiting", conclusion: null, outcome: "blocked" });

/** A run as the core opens it; what it works out from the jobs is the test's to say. */
function opened(run: Run, jobs: Job[] = [], worked: Partial<OpenRun> = {}): OpenRun {
    return {
        run,
        jobs: jobs.map((job) => ({ job, stoppedAt: null })),
        attempts: run.attempt,
        unfinished: run.status !== "completed",
        done: 0,
        failed: 0,
        failedJob: null,
        graph: { columns: [], edges: [] },
        timing: null,
        summaries: [],
        ...worked,
    };
}

const makeJob = (overrides: Partial<Job> = {}): Job => ({
    id: "3",
    name: "build",
    status: "completed",
    conclusion: "success",
    outcome: "success",
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
    api.artifacts.mockReset().mockResolvedValue([]);
    api.rerun.mockReset().mockResolvedValue(undefined);
    api.rerunJob.mockReset().mockResolvedValue(undefined);
    api.annotations.mockReset().mockResolvedValue([]);
    api.jobSummary.mockReset().mockResolvedValue(null);
    api.workflowFile.mockReset().mockResolvedValue({ path: ".github/workflows/ci.yml", text: "on: push" });
    api.deleteRun.mockReset().mockResolvedValue(undefined);
    api.deleteRunLogs.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
    cleanup();
});

async function renderRun(run: Run, runId = run.id) {
    runs.mockReset().mockImplementation(({ runId: id, attempt }) =>
        opened({ ...run, id: id as string, attempt: (attempt as number | null) ?? run.attempt }, [], { attempts: run.attempt }),
    );
    const view = render(<RunView paneId="pane" project={null} repo={repo} runId={runId} openJob={null} active canWrite />, { wrapper });
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
        render(<RunMenu run={waiting} unfinished repo={repo} canWrite onDeleted={() => {}} />, { wrapper });
        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        expect(screen.getByRole("menuitem", { name: "Copy link" })).toBeTruthy();
        expect(screen.queryByRole("menuitem", { name: "Re-run all with debug logs" })).toBeNull();
        expect(screen.queryByRole("menuitem", { name: "Delete all logs" })).toBeNull();
        expect(screen.queryByRole("menuitem", { name: "Delete run" })).toBeNull();
    });

    it("is read again when the core says it moved", async () => {
        await renderRun(waiting);
        expect(runs).toHaveBeenCalledTimes(1);
        await act(async () => core.listsMoved(["run"]));
        expect(runs).toHaveBeenCalledTimes(2);
    });
});

describe("RunView", () => {
    it("re-runs once however many times the button is pressed", async () => {
        await renderRun(makeRun());
        const button = screen.getByRole("button", { name: "Re-run all" });
        fireEvent.click(button);
        fireEvent.click(button);
        await act(async () => {});
        expect(api.rerun).toHaveBeenCalledTimes(1);
    });

    it("asks a newly opened run for its latest attempt, not the attempt picked on the one before", async () => {
        const view = await renderRun(makeRun({ attempt: 2 }));
        fireEvent.click(screen.getByRole("button", { name: "Attempt 1" }));
        await act(async () => {});
        expect(runs).toHaveBeenLastCalledWith(expect.objectContaining({ runId: "7", attempt: 1 }));

        view.rerender(<RunView paneId="pane" project={null} repo={repo} runId="8" openJob={null} active canWrite />);
        await act(async () => {});
        expect(runs).toHaveBeenLastCalledWith(expect.objectContaining({ runId: "8", attempt: null }));
    });
});

function Pane({ canWrite = true }: { canWrite?: boolean }) {
    const view = useHostView("pane");
    return <RunView paneId="pane" project={null} repo={repo} runId="7" openJob={view.job} active canWrite={canWrite} />;
}

async function renderPane(run: Run, jobs: Job[] = [], canWrite = true, worked: Partial<OpenRun> = {}) {
    runs.mockReset().mockReturnValue(opened(run, jobs, worked));
    const view = render(<Pane canWrite={canWrite} />, { wrapper });
    await act(async () => {});
    return view;
}

describe("the run's card", () => {
    it("cancels a run only once the person confirms", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null, outcome: "running" }));
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
        await renderPane(makeRun({ status: "in_progress", conclusion: null, outcome: "running" }), [], false);
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
        await renderPane(
            makeRun(),
            [makeJob({ id: "1", name: "lint" }), makeJob({ id: "2", name: "test", conclusion: "failure", outcome: "failure" })],
            true,
            { failedJob: "2" },
        );
        expect(viewOf("pane")).toMatchObject({ pickFailed: false, job: "2", runTab: "logs" });
        expect(screen.getByRole("tab", { name: "Logs · test" }).getAttribute("aria-selected")).toBe("true");
    });

    it("stays on the summary when no job failed", async () => {
        updateView("pane", { pickFailed: true });
        await renderPane(makeRun({ conclusion: "success", outcome: "success" }), [makeJob()]);
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
        fireEvent.click(screen.getByRole("button", { name: "With debug logs" }));
        await act(async () => {});
        expect(api.rerunJob).toHaveBeenLastCalledWith(repo, "3", true);
    });

    it("offers no re-run of a job still going, or to someone who cannot write", async () => {
        await openJob(makeJob({ status: "in_progress", conclusion: null, outcome: "running", completedAt: null }));
        expect(screen.queryByRole("button", { name: "Re-run this job" })).toBeNull();
        cleanup();
        await openJob(makeJob(), false);
        expect(screen.queryByRole("button", { name: "Re-run this job" })).toBeNull();
        expect(screen.queryByRole("button", { name: "With debug logs" })).toBeNull();
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

describe("a run still going", () => {
    it("shows what the core read last once it says the run moved", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null, outcome: "running" }));
        runs.mockReturnValue(opened(makeRun({ status: "in_progress", conclusion: null, outcome: "running", title: "Ship it now" })));
        await act(async () => core.listsMoved(["run"]));
        expect(screen.getByRole("heading").textContent).toBe("Ship it now");
    });

    it("is not read again for a change to another list", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null, outcome: "running" }));
        await act(async () => core.listsMoved(["pulls"]));
        expect(runs).toHaveBeenCalledTimes(1);
    });
});

describe("the run's menu", () => {
    async function openMenu(run: Run = makeRun(), canWrite = true) {
        const onDeleted = vi.fn();
        render(<RunMenu run={run} unfinished={run.status !== "completed"} repo={repo} canWrite={canWrite} onDeleted={onDeleted} />, { wrapper });
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
