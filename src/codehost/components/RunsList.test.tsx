import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Run, RunPage, Workflow } from "../api";

const api = vi.hoisted(() => ({
    runs: vi.fn(),
    workflows: vi.fn(),
    pulls: vi.fn(),
}));

import { invalidate } from "../../plugin-api/resources";
import { hostSettings, resetView, updateView, useHostView, viewOf } from "../state";
import { ciCore, InHost, registerTestHost, TEST_HOST } from "../testHost";
import { RunsList } from "./RunsList";
import { setState } from "../../state/store";
import { testPreferences } from "../../test/preferences";

const host = registerTestHost(api);
const core = ciCore(api);
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
    outcome: "success",
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
                project="/repo"
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

beforeEach(() => {
    setState({ preferences: testPreferences() });
    invalidate(() => true);
    resetView("pane");
    hostSettings(TEST_HOST).update((settings) => ({ ...settings, followBranch: true }));
    api.runs.mockReset().mockResolvedValue(pageOf([makeRun()]));
    api.workflows
        .mockReset()
        .mockResolvedValue([workflow(), workflow({ id: "2", name: "Nightly", path: ".github/workflows/nightly.yml", active: false })]);
    api.pulls.mockReset().mockResolvedValue([]);
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("filtering runs", () => {
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

    it("starts a different branch from its first page", async () => {
        const view = await renderList({ branch: "main" });
        act(() => updateView("pane", { page: 3, run: "7" }));
        view.rerender(<Harness branch="dev" />);
        await act(async () => {});
        expect(viewOf("pane")).toMatchObject({ page: 1, run: null });
    });
});

describe("keeping the list fresh", () => {
    it("reads again when the core says the lists moved, and only then", async () => {
        api.runs.mockResolvedValue(pageOf([makeRun({ status: "in_progress", conclusion: null, outcome: "running" })]));
        await renderList();
        const before = api.runs.mock.calls.length;
        await act(async () => {
            core.moved(repo);
        });
        expect(api.runs.mock.calls.length).toBe(before);
        await act(async () => {
            core.listsMoved();
        });
        expect(api.runs.mock.calls.length).toBeGreaterThan(before);
        expect(document.querySelector(".gha-run-item")).toHaveAttribute("data-outcome", "running");
    });
});
