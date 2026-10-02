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
    downloadArtifact: vi.fn(),
    deleteRun: vi.fn(),
    deleteRunLogs: vi.fn(),
}));

const shell = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()), copyText: vi.fn(() => Promise.resolve()) }));

vi.mock("../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    openUrl: shell.openUrl,
    copyText: shell.copyText,
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
    for (const mock of Object.values(shell)) mock.mockClear();
    api.runTiming.mockReset().mockResolvedValue({ runDurationMs: null, billable: [] });
    api.artifacts.mockReset().mockResolvedValue([]);
    api.rerun.mockReset().mockResolvedValue(undefined);
    api.rerunJob.mockReset().mockResolvedValue(undefined);
    api.annotations.mockReset().mockResolvedValue([]);
    api.jobSummary.mockReset().mockResolvedValue(null);
    api.workflowFile.mockReset().mockResolvedValue({ path: ".github/workflows/ci.yml", text: "on: push" });
    api.downloadArtifact.mockReset();
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

    it("re-runs only the failed jobs of a run that did not pass", async () => {
        await renderPane(makeRun());
        fireEvent.click(screen.getByRole("button", { name: "Re-run failed jobs" }));
        await act(async () => {});
        expect(api.rerun).toHaveBeenCalledWith(repo, "7", true);
        expect(toasts()).toContain("Re-running the failed jobs");
    });

    it("offers no re-run of failed jobs on a run that passed", async () => {
        await renderPane(makeRun({ conclusion: "success" }));
        expect(screen.queryByRole("button", { name: "Re-run failed jobs" })).toBeNull();
        expect(screen.getByRole("button", { name: "Re-run all jobs" })).toBeTruthy();
    });

    it("says why a re-run failed", async () => {
        api.rerun.mockRejectedValue(new Error("no permission"));
        await renderPane(makeRun());
        fireEvent.click(screen.getByRole("button", { name: "Re-run all jobs" }));
        await act(async () => {});
        expect(toasts().some((text) => text.endsWith("no permission"))).toBe(true);
    });

    it("offers nothing that changes the run to someone who cannot write", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null }), [], false);
        expect(screen.queryByRole("button", { name: "Cancel run" })).toBeNull();
        cleanup();
        await renderPane(makeRun(), [], false);
        expect(screen.queryByRole("button", { name: "Re-run all jobs" })).toBeNull();
        expect(screen.queryByRole("button", { name: "Re-run failed jobs" })).toBeNull();
    });

    it("takes a finished run's duration from the host, and says what it billed", async () => {
        api.runTiming.mockResolvedValue({ runDurationMs: 90_000, billable: [{ runner: "UBUNTU", totalMs: 61_000, jobs: 2 }] });
        await renderPane(makeRun());
        expect(within(card()).getByText("1m 30s")).toBeTruthy();
        expect(screen.getByText("Billed 2 min").getAttribute("title")).toBe("UBUNTU: 1m 1s over 2 jobs");
    });

    it("measures a finished run from its own clock when the host has no timing", async () => {
        await renderPane(makeRun());
        expect(within(card()).getByText("5m 0s")).toBeTruthy();
        expect(screen.queryByText(/^Billed/)).toBeNull();
    });

    it("goes back to the latest attempt when it is picked again", async () => {
        await renderPane(makeRun({ attempt: 2 }));
        fireEvent.click(screen.getByRole("button", { name: "#1" }));
        await act(async () => {});
        expect(screen.getByRole("button", { name: "#1" }).dataset.on).toBe("1");
        api.runAttempt.mockClear();
        fireEvent.click(screen.getByRole("button", { name: "#2" }));
        await act(async () => {});
        expect(screen.getByRole("button", { name: "#2" }).dataset.on).toBe("1");
        expect(api.runAttempt).not.toHaveBeenCalled();
    });

    it("reads the run again when Refresh is pressed", async () => {
        await renderPane(makeRun());
        api.run.mockClear();
        fireEvent.click(screen.getByRole("button", { name: "Refresh run" }));
        await act(async () => {});
        expect(api.run).toHaveBeenCalledWith(repo, "7");
    });
});

describe("the run's heading", () => {
    it("reads who started it, how, on which commit, branch and pull request, and which attempt", async () => {
        await renderPane(makeRun({ actor: "someone", attempt: 2, pullRequests: [31], event: "workflow_dispatch", title: "Ship it", name: "CI" }));
        const meta = document.querySelector(".git-detail-meta") as HTMLElement;
        expect(within(meta).getByText("someone")).toBeTruthy();
        expect(within(meta).getByText(/^Manual /)).toBeTruthy();
        expect(within(meta).getByText("main")).toBeTruthy();
        expect(within(meta).getByText("#31")).toBeTruthy();
        expect(within(meta).getByText("CI")).toBeTruthy();
        expect(within(meta).getByText("attempt 2")).toBeTruthy();
    });

    it("falls back to the workflow's name for a run with no title, without naming it twice", async () => {
        await renderPane(makeRun({ title: "", name: "CI" }));
        expect(screen.getByRole("heading").textContent).toBe("CI #12");
    });

    it("copies the full commit and opens the run on the host", async () => {
        await renderPane(makeRun({ sha: "abc123456789", shortSha: "abc1234" }));
        fireEvent.click(screen.getByRole("button", { name: "abc1234" }));
        await act(async () => {});
        expect(shell.copyText).toHaveBeenCalledWith("abc123456789");
        expect(toasts()).toContain("Copied abc1234");
        fireEvent.click(screen.getByRole("button", { name: "Open on Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://github.com/nodelike/sikemux/actions/runs/7");
    });
});

describe("a run that cannot be shown", () => {
    it("shows placeholders while it loads", async () => {
        let answer: (detail: { run: Run; jobs: Job[] }) => void = () => {};
        api.run.mockReset().mockReturnValue(new Promise((resolve) => (answer = resolve)));
        render(<Pane />, { wrapper });
        await act(async () => {});
        expect(screen.getByLabelText("Loading run")).toBeTruthy();
        await act(async () => answer({ run: makeRun(), jobs: [] }));
        expect(screen.queryByLabelText("Loading run")).toBeNull();
    });

    it("says why it could not be read, and tries again on request", async () => {
        api.run.mockReset().mockRejectedValueOnce("Not Found").mockResolvedValue({ run: makeRun(), jobs: [] });
        render(<Pane />, { wrapper });
        await act(async () => {});
        expect(screen.getByText("Could not read the run")).toBeTruthy();
        expect(screen.getByText("Not Found")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await act(async () => {});
        expect(screen.getByRole("heading").textContent).toBe("Ship it #12");
    });
});

describe("going back", () => {
    it("leads to the runs list when the run was opened from there", async () => {
        updateView("pane", { run: "7" });
        await renderPane(makeRun());
        fireEvent.click(screen.getByRole("button", { name: "Runs" }));
        expect(viewOf("pane").run).toBeNull();
    });

    it("leads to the pull request whose check opened the run", async () => {
        updateView("pane", { run: "7", runFrom: 31 });
        await renderPane(makeRun());
        fireEvent.click(screen.getByRole("button", { name: "#31" }));
        expect(viewOf("pane")).toMatchObject({ run: null, item: 31, runFrom: null });
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

describe("the run's jobs", () => {
    it("lists each job with how long it took, where it stopped, and which were skipped", async () => {
        await renderPane(makeRun(), [
            makeJob({ id: "1", name: "lint" }),
            makeJob({
                id: "2",
                name: "test",
                conclusion: "failure",
                steps: [
                    { number: 1, name: "Set up", status: "completed", conclusion: "success", startedAt: null, completedAt: null },
                    { number: 2, name: "Run tests", status: "completed", conclusion: "failure", startedAt: null, completedAt: null },
                ],
            }),
            makeJob({ id: "3", name: "deploy", conclusion: "skipped", startedAt: null, completedAt: null }),
        ]);
        const rows = document.querySelector(".pr-files") as HTMLElement;
        expect(within(rows).getByText("3")).toBeTruthy();
        expect(within(rows).getByText("at Run tests")).toBeTruthy();
        expect(within(rows).getByText("skipped")).toBeTruthy();
        expect(within(rows).getAllByText("1m 5s")).toHaveLength(2);
    });

    it("says there are none yet", async () => {
        await renderPane(makeRun({ status: "queued", conclusion: null }));
        expect(within(document.querySelector(".pr-files") as HTMLElement).getByText("No jobs yet")).toBeTruthy();
    });

    it("opens a job's log from its row and marks the row", async () => {
        await renderPane(makeRun(), [makeJob({ runner: "ubuntu-latest" })]);
        fireEvent.click(within(document.querySelector(".pr-files") as HTMLElement).getByText("build"));
        expect(viewOf("pane")).toMatchObject({ job: "3", runTab: "logs" });
        expect(document.querySelector(".git-file-row.sel")).toBeTruthy();
        expect(screen.getByText("on ubuntu-latest")).toBeTruthy();
    });

    it("asks for a job to be picked before showing a log", async () => {
        await renderPane(makeRun(), [makeJob()]);
        fireEvent.click(screen.getByRole("tab", { name: "Logs" }));
        expect(screen.getByText("Pick a job on the left to read its log.")).toBeTruthy();
        fireEvent.click(screen.getByRole("tab", { name: "Summary" }));
        expect(viewOf("pane").runTab).toBe("summary");
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

    it("says why a job could not be re-run", async () => {
        api.rerunJob.mockRejectedValue(new Error("gone"));
        await openJob(makeJob());
        fireEvent.click(screen.getByRole("button", { name: "Re-run this job" }));
        await act(async () => {});
        expect(toasts()).toContain("Could not re-run build: gone");
    });

    it("offers no re-run of a job still going, or to someone who cannot write", async () => {
        await openJob(makeJob({ status: "in_progress", conclusion: null, completedAt: null }));
        expect(screen.queryByRole("button", { name: "Re-run this job" })).toBeNull();
        cleanup();
        await openJob(makeJob(), false);
        expect(screen.queryByRole("button", { name: "Re-run this job" })).toBeNull();
        expect(screen.queryByRole("button", { name: "with debug logs" })).toBeNull();
    });

    it("opens the job on the host when it has a page there", async () => {
        await openJob(makeJob({ url: "https://github.com/nodelike/sikemux/actions/runs/7/job/3" }));
        fireEvent.click(screen.getByRole("button", { name: "On Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://github.com/nodelike/sikemux/actions/runs/7/job/3");
    });

    it("lists the job's steps", async () => {
        await openJob(
            makeJob({
                steps: [{ number: 1, name: "Checkout", status: "completed", conclusion: "success", startedAt: null, completedAt: null }],
            }),
        );
        const steps = screen.getAllByTitle("Show this step in the log");
        expect(steps.map((step) => step.textContent)).toEqual(["Checkout—"]);
    });

    it("shows what CI flagged in the job, where it was checked", async () => {
        api.annotations.mockResolvedValue([
            { path: "src/app.ts", startLine: 4, endLine: 4, level: "failure", title: "Type error", message: "boom", details: null },
        ]);
        await openJob(makeJob({ checkRunId: "c1" }));
        expect(api.annotations).toHaveBeenCalledWith(repo, "c1");
        expect(screen.getByText("boom")).toBeTruthy();
        expect(screen.getByText("src/app.ts:4")).toBeTruthy();
    });

    it("does not ask for annotations of a job that was never checked", async () => {
        await openJob(makeJob());
        expect(api.annotations).not.toHaveBeenCalled();
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

    it("says why the workflow file could not be read", async () => {
        api.workflowFile.mockRejectedValue("Not Found");
        await renderPane(makeRun({ path: ".github/workflows/ci.yml" }));
        fireEvent.click(screen.getByRole("button", { name: "Workflow file" }));
        await act(async () => {});
        expect(screen.getByText("Not Found")).toBeTruthy();
    });

    it("offers no workflow file for a run with no file behind it", async () => {
        await renderPane(makeRun({ path: "dynamic/pages/pages-build-deployment" }));
        expect(screen.queryByRole("button", { name: "Workflow file" })).toBeNull();
    });

    it("reads the first ten jobs' summaries, and the rest only when asked", async () => {
        api.jobSummary.mockImplementation((_repo, checkRunId: string) =>
            Promise.resolve(checkRunId === "c0" ? { title: "", body: "All green" } : null),
        );
        const jobs = Array.from({ length: 12 }, (_, index) => makeJob({ id: String(index), name: `job ${index}`, checkRunId: `c${index}` }));
        await renderPane(makeRun(), jobs);
        expect(api.jobSummary).toHaveBeenCalledTimes(10);
        expect(screen.getByText("job 0 summary")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Look for summaries from 2 more jobs" }));
        await act(async () => {});
        expect(api.jobSummary).toHaveBeenCalledTimes(12);
        expect(screen.queryByRole("button", { name: /Look for summaries/ })).toBeNull();
    });

    it("counts one job left over as one job", async () => {
        const jobs = Array.from({ length: 11 }, (_, index) => makeJob({ id: String(index), checkRunId: `c${index}` }));
        await renderPane(makeRun(), jobs);
        expect(screen.getByRole("button", { name: "Look for summaries from 1 more job" })).toBeTruthy();
    });

    it("reads no summaries while the run is still going", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null }), [makeJob({ checkRunId: "c1" })]);
        expect(api.jobSummary).not.toHaveBeenCalled();
    });
});

describe("the run's artifacts", () => {
    const artifact = (id: string, name: string, expired = false) => ({ id, name, sizeBytes: 2048, expired, createdAt: null, expiresAt: null });

    it("folds away under the jobs and downloads one on request", async () => {
        api.artifacts.mockResolvedValue([artifact("a1", "dist"), artifact("a2", "coverage", true)]);
        let saved: (value: { path: string; bytes: number }) => void = () => {};
        api.downloadArtifact.mockReturnValue(new Promise((resolve) => (saved = resolve)));
        await renderPane(makeRun());
        const toggle = screen.getByRole("button", { name: /Artifacts/ });
        expect(toggle.textContent).toContain("2");
        expect(screen.queryByText("dist")).toBeNull();
        fireEvent.click(toggle);

        expect(screen.getByText("expired")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Download" }));
        expect(api.downloadArtifact).toHaveBeenCalledWith(repo, "a1", "dist");
        expect(screen.getByRole("button", { name: "Saving…" })).toHaveProperty("disabled", true);
        await act(async () => saved({ path: "/Downloads/dist.zip", bytes: 2048 }));
        expect(toasts()).toContain("Saved dist to /Downloads/dist.zip");
        expect(screen.getByRole("button", { name: "Download" })).toHaveProperty("disabled", false);
    });

    it("says why a download failed", async () => {
        api.artifacts.mockResolvedValue([artifact("a1", "dist")]);
        api.downloadArtifact.mockRejectedValue(new Error("disk full"));
        await renderPane(makeRun());
        fireEvent.click(screen.getByRole("button", { name: /Artifacts/ }));
        fireEvent.click(screen.getByRole("button", { name: "Download" }));
        await act(async () => {});
        expect(toasts()).toContain("Could not download dist: disk full");
    });

    it("does not look for artifacts until the run has finished", async () => {
        await renderPane(makeRun({ status: "in_progress", conclusion: null }));
        expect(api.artifacts).not.toHaveBeenCalled();
        expect(screen.queryByRole("button", { name: /Artifacts/ })).toBeNull();
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

    it("says why the watch could not start", async () => {
        api.watchStart.mockRejectedValue("github: rate limited");
        await renderPane(going());
        expect(screen.getByText("github: rate limited")).toBeTruthy();
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

    it("re-runs everything with debug logs", async () => {
        await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Re-run all with debug logs" }));
        await act(async () => {});
        expect(api.rerun).toHaveBeenCalledWith(repo, "7", false, true);
        expect(toasts()).toContain("Re-running everything with debug logs");
        expect(screen.queryByRole("menu")).toBeNull();
    });

    it("says why a debug re-run failed", async () => {
        api.rerun.mockRejectedValue(new Error("nope"));
        await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Re-run all with debug logs" }));
        await act(async () => {});
        expect(toasts()).toContain("Could not re-run it: nope");
    });

    it("copies the run's link and opens it on the host", async () => {
        await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Copy link" }));
        await act(async () => {});
        expect(shell.copyText).toHaveBeenCalledWith("https://github.com/nodelike/sikemux/actions/runs/7");
        expect(toasts()).toContain("Copied the link");
        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Open on Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://github.com/nodelike/sikemux/actions/runs/7");
    });

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

    it("closes on Escape or a click outside", async () => {
        await openMenu();
        fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
        expect(screen.queryByRole("menu")).toBeNull();
        fireEvent.click(screen.getByRole("button", { name: "More run actions" }));
        fireEvent.click(document.querySelector(".env-dd-scrim") as HTMLElement);
        expect(screen.queryByRole("menu")).toBeNull();
    });
});
