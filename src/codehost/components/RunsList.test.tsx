import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pull, Run, RunPage, Workflow } from "../api";

const api = vi.hoisted(() => ({
    runs: vi.fn(),
    workflows: vi.fn(),
    pulls: vi.fn(),
}));

const shell = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()) }));

vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl: shell.openUrl }));

import { invalidate } from "../../plugin-api/resources";
import { hostSettings, resetView, updateView, useHostView, viewOf } from "../state";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { RunsList } from "./RunsList";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const makeRun = (overrides: Partial<Run> = {}): Run => ({
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
    sha: "abc123def",
    shortSha: "abc123d",
    actor: "someone",
    avatarUrl: null,
    createdAt: "2026-01-01T12:00:00Z",
    startedAt: "2026-01-01T12:00:00Z",
    updatedAt: "2026-01-01T12:05:00Z",
    pullRequests: [],
    url: "https://github.com/nodelike/sikemux/actions/runs/7/attempts/1",
    ...overrides,
});

const workflow = (overrides: Partial<Workflow> = {}): Workflow => ({
    id: "1",
    name: "CI",
    path: ".github/workflows/ci.yml",
    state: "active",
    active: true,
    url: "",
    ...overrides,
});

const pageOf = (runs: Run[], overrides: Partial<RunPage> = {}): RunPage => ({ runs, total: runs.length, nextPage: null, ...overrides });

function Harness({
    branch = null,
    projectBranch = null,
    canWrite = true,
    onDispatch = () => {},
}: {
    branch?: string | null;
    projectBranch?: string | null;
    canWrite?: boolean;
    onDispatch?: (workflowId: string) => void;
}) {
    const view = useHostView("pane");
    return (
        <InHost host={host}>
            <RunsList
                paneId="pane"
                repo={repo}
                view={view}
                branch={branch}
                projectBranch={projectBranch}
                active
                canWrite={canWrite}
                onDispatch={onDispatch}
            />
        </InHost>
    );
}

async function renderList(props: Parameters<typeof Harness>[0] = {}) {
    const view = render(<Harness {...props} />);
    await act(async () => {});
    return view;
}

const lastQuery = () => api.runs.mock.calls.at(-1)?.[0];

beforeEach(() => {
    invalidate(() => true);
    resetView("pane");
    hostSettings(TEST_HOST).update((settings) => ({ ...settings, followBranch: true }));
    api.runs.mockReset().mockResolvedValue(pageOf([makeRun()]));
    api.workflows
        .mockReset()
        .mockResolvedValue([workflow(), workflow({ id: "2", name: "Nightly", path: ".github/workflows/nightly.yml", active: false })]);
    api.pulls.mockReset().mockResolvedValue([]);
    shell.openUrl.mockClear();
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("a run's row", () => {
    it("names the run by its title, then its workflow, then its number", async () => {
        api.runs.mockResolvedValue(
            pageOf([makeRun({ id: "1" }), makeRun({ id: "2", title: "", name: "Deploy" }), makeRun({ id: "3", title: "", name: "", runNumber: 40 })]),
        );
        await renderList();
        expect(screen.getByText("Ship it")).toBeTruthy();
        expect(screen.getByText("Deploy")).toBeTruthy();
        expect(screen.getByText("Run #40")).toBeTruthy();
    });

    it("says how the run went in words", async () => {
        api.runs.mockResolvedValue(
            pageOf([
                makeRun({ id: "1", title: "a" }),
                makeRun({ id: "2", title: "b", conclusion: "timed_out" }),
                makeRun({ id: "3", title: "c", status: "in_progress", conclusion: null }),
                makeRun({ id: "4", title: "d", status: "waiting", conclusion: null }),
            ]),
        );
        await renderList();
        const words = [...document.querySelectorAll(".gha-run-status-word")].map((node) => node.textContent);
        expect(words).toEqual(["Passed", "Failed", "Running", "Waiting for approval"]);
    });

    it("names the pull request a run belongs to, and falls back to its number when the title is unknown", async () => {
        api.pulls.mockResolvedValue([{ number: 4, title: "the run page" } as Pull]);
        api.runs.mockResolvedValue(
            pageOf([makeRun({ id: "1", title: "a", pullRequests: [4] }), makeRun({ id: "2", title: "b", pullRequests: [8, 9], branch: "feat/x" })]),
        );
        await renderList();
        expect(screen.getByText("the run page")).toBeTruthy();
        expect(screen.getByText("#8 #9")).toBeTruthy();
        expect(screen.queryByText("feat/x")).toBeNull();
    });

    it("shows the branch when no pull request is linked, and leaves out what it does not know", async () => {
        api.runs.mockResolvedValue(pageOf([makeRun({ actor: null, shortSha: "", branch: "dev" })]));
        await renderList();
        expect(screen.getByText("dev")).toBeTruthy();
        expect(screen.getByText("#12 - someone")).toBeTruthy();
        expect(document.querySelector(".gha-run-ref:not(.gha-run-branch)")).toBeNull();
    });

    it("titles a row with its workflow only while every workflow is listed", async () => {
        await renderList();
        expect(document.querySelector(".gha-run-row")?.getAttribute("title")).toBe("CI #12");
        act(() => updateView("pane", { workflowId: "1" }));
        await act(async () => {});
        expect(document.querySelector(".gha-run-row")?.getAttribute("title")).toBeNull();
    });

    it("opens the run in the pane when clicked", async () => {
        await renderList();
        fireEvent.click(screen.getByText("Ship it"));
        expect(viewOf("pane").run).toBe("7");
    });
});

describe("filtering runs", () => {
    it("asks for one status at a time, and nothing narrower for All", async () => {
        await renderList();
        expect(lastQuery()).toMatchObject({ status: undefined, page: 1, perPage: 30 });
        fireEvent.click(screen.getByRole("button", { name: "Failed" }));
        await act(async () => {});
        expect(lastQuery()).toMatchObject({ status: "failure" });
        expect(screen.getByRole("button", { name: "Failed" }).dataset.on).toBe("1");
        fireEvent.click(screen.getByRole("button", { name: "All" }));
        expect(viewOf("pane").statusFilter).toBe("all");
        expect(screen.getByRole("button", { name: "All" }).dataset.on).toBe("1");
    });

    it("narrows to one workflow, marks the ones switched off, and goes back to every workflow", async () => {
        await renderList();
        fireEvent.click(screen.getByRole("button", { name: "Which workflow's runs to show" }));
        const options = screen.getAllByRole("option");
        expect(options.map((option) => option.textContent)).toEqual(["Every workflow", "CI", "Nightlyoff"]);
        fireEvent.click(options[2]);
        await act(async () => {});
        expect(lastQuery()).toMatchObject({ workflowId: "2" });

        fireEvent.click(screen.getByRole("button", { name: "Which workflow's runs to show" }));
        fireEvent.click(screen.getByRole("option", { name: "Every workflow" }));
        await act(async () => {});
        expect(viewOf("pane").workflowId).toBeNull();
    });

    it("offers to start a workflow only when it is picked, switched on, and the repository can be written to", async () => {
        const onDispatch = vi.fn();
        const view = await renderList({ onDispatch });
        expect(screen.queryByRole("button", { name: "Run workflow" })).toBeNull();

        act(() => updateView("pane", { workflowId: "2" }));
        await act(async () => {});
        expect(screen.queryByRole("button", { name: "Run workflow" })).toBeNull();

        act(() => updateView("pane", { workflowId: "1" }));
        await act(async () => {});
        fireEvent.click(screen.getByRole("button", { name: "Run workflow" }));
        expect(onDispatch).toHaveBeenCalledWith("1");

        view.rerender(<Harness canWrite={false} onDispatch={onDispatch} />);
        expect(screen.queryByRole("button", { name: "Run workflow" })).toBeNull();
    });

    it("lets the list follow the project's branch unless a branch was typed", async () => {
        const view = await renderList({ projectBranch: "feat/x" });
        const chip = screen.getByRole("button", { name: "This branch" });
        expect(chip.dataset.on).toBe("1");
        fireEvent.click(chip);
        expect(hostSettings(TEST_HOST).get().followBranch).toBe(false);
        expect(screen.getByRole("button", { name: "This branch" }).dataset.on).toBe("0");

        act(() => updateView("pane", { branch: "typed" }));
        view.rerender(<Harness projectBranch="feat/x" branch="typed" />);
        expect(screen.queryByRole("button", { name: "This branch" })).toBeNull();
    });

    it("counts the runs and names the branch they are on", async () => {
        api.runs.mockResolvedValue(pageOf([makeRun()], { total: 1 }));
        const view = await renderList();
        expect(document.querySelector(".gha-list-head > .gha-dim")?.textContent).toBe("1 run");
        api.runs.mockResolvedValue(pageOf([makeRun()], { total: 57 }));
        view.rerender(<Harness branch="main" />);
        await act(async () => {});
        expect(lastQuery()).toMatchObject({ branch: "main" });
        expect(document.querySelector(".gha-list-head > .gha-dim")?.textContent).toBe("57 runs on main");
    });

    it("starts a different branch from its first page", async () => {
        const view = await renderList({ branch: "main" });
        act(() => updateView("pane", { page: 3, run: "7" }));
        view.rerender(<Harness branch="dev" />);
        await act(async () => {});
        expect(viewOf("pane")).toMatchObject({ page: 1, run: null });
    });
});

describe("what the list says when there is nothing to show", () => {
    it("shows placeholders while the first page loads", async () => {
        let answer: (page: RunPage) => void = () => {};
        api.runs.mockReturnValue(new Promise((resolve) => (answer = resolve)));
        await renderList();
        expect(screen.getByLabelText("Loading runs")).toBeTruthy();
        await act(async () => answer(pageOf([makeRun()])));
        expect(screen.queryByLabelText("Loading runs")).toBeNull();
    });

    it("says why the runs could not be read, and reads them again on request", async () => {
        api.runs.mockRejectedValueOnce("GitHub is down").mockResolvedValue(pageOf([makeRun()]));
        await renderList();
        expect(screen.getByText("Could not read runs")).toBeTruthy();
        expect(screen.getByText("GitHub is down")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await act(async () => {});
        expect(screen.getByText("Ship it")).toBeTruthy();
    });

    it("says nothing matches, naming the branch it looked on", async () => {
        api.runs.mockResolvedValue(pageOf([]));
        await renderList({ branch: "dev" });
        expect(screen.getByText("Nothing matches this filter on dev.")).toBeTruthy();
        expect(screen.queryByRole("button", { name: /Open on/ })).toBeNull();
    });

    it("says nothing matches without a branch when none was asked for", async () => {
        api.runs.mockResolvedValue(pageOf([]));
        await renderList();
        expect(screen.getByText("Nothing matches this filter.")).toBeTruthy();
    });
});

describe("paging and leaving the list", () => {
    it("has no pager when everything fits on one page", async () => {
        await renderList();
        expect(screen.queryByRole("button", { name: "Older" })).toBeNull();
    });

    it("steps to older runs and back", async () => {
        api.runs.mockResolvedValue(pageOf([makeRun()], { nextPage: 2 }));
        await renderList();
        expect(screen.getByRole("button", { name: "Newer" })).toHaveProperty("disabled", true);
        fireEvent.click(screen.getByRole("button", { name: "Older" }));
        await act(async () => {});
        expect(lastQuery()).toMatchObject({ page: 2 });

        api.runs.mockResolvedValue(pageOf([makeRun()], { nextPage: null }));
        act(() => updateView("pane", { page: 3 }));
        await act(async () => {});
        expect(screen.getByText("Page 3")).toBeTruthy();
        expect(screen.getByRole("button", { name: "Older" })).toHaveProperty("disabled", true);
        fireEvent.click(screen.getByRole("button", { name: "Newer" }));
        expect(viewOf("pane").page).toBe(2);
    });

    it("opens the repository's Actions page, or the picked workflow's", async () => {
        await renderList();
        fireEvent.click(screen.getByRole("button", { name: "Open on Test host" }));
        expect(shell.openUrl).toHaveBeenLastCalledWith("https://github.com/nodelike/sikemux/actions");

        act(() => updateView("pane", { workflowId: "1" }));
        await act(async () => {});
        fireEvent.click(screen.getByRole("button", { name: "Open on Test host" }));
        expect(shell.openUrl).toHaveBeenLastCalledWith("https://github.com/nodelike/sikemux/actions/workflows/ci.yml");
    });

    it("reads the list again when Refresh is pressed", async () => {
        await renderList();
        const before = api.runs.mock.calls.length;
        fireEvent.click(screen.getByRole("button", { name: "Refresh runs" }));
        await act(async () => {});
        expect(api.runs.mock.calls.length).toBe(before + 1);
    });
});

describe("keeping the list fresh", () => {
    it("reads again every ten seconds while a run is going", async () => {
        vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
        api.runs.mockResolvedValue(pageOf([makeRun({ status: "in_progress", conclusion: null })]));
        await renderList();
        const before = api.runs.mock.calls.length;
        await act(async () => {
            vi.advanceTimersByTime(10_000);
        });
        expect(api.runs.mock.calls.length).toBe(before + 1);
        expect(within(document.querySelector(".gha-run-status") as HTMLElement).getByText("Running")).toBeTruthy();
    });

    it("waits twenty seconds between reads once everything has finished", async () => {
        vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
        await renderList();
        const before = api.runs.mock.calls.length;
        await act(async () => {
            vi.advanceTimersByTime(10_000);
        });
        expect(api.runs.mock.calls.length).toBe(before);
        await act(async () => {
            vi.advanceTimersByTime(10_000);
        });
        expect(api.runs.mock.calls.length).toBe(before + 1);
    });
});
