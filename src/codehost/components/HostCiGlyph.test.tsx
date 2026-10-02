import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Run } from "../api";
import type { HostRepo } from "../project";

const api = vi.hoisted(() => ({ status: vi.fn(), runs: vi.fn() }));
const project = vi.hoisted(() => ({ found: { repo: null, remote: null, branch: null, loading: false } as HostRepo }));
const commands = vi.hoisted(() => ({ openGitArea: vi.fn() }));

vi.mock("../project", async (importOriginal) => ({ ...(await importOriginal<object>()), useHostRepo: () => project.found }));
vi.mock("../../state/commands", async (importOriginal) => ({ ...(await importOriginal<object>()), openGitArea: commands.openGitArea }));

import { invalidate } from "../../plugin-api/resources";
import { registerTestHost, TEST_HOST } from "../testHost";
import { hostCiGlyph } from "./HostCiGlyph";

registerTestHost(api);
const Glyph = hostCiGlyph(TEST_HOST);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux", account: "work" };

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
    sha: "abc",
    shortSha: "abc",
    actor: null,
    avatarUrl: null,
    createdAt: new Date(Date.now() - 5 * 60_000).toISOString(),
    startedAt: null,
    updatedAt: new Date().toISOString(),
    pullRequests: [],
    url: "",
    ...overrides,
});

async function renderGlyph(found: Partial<HostRepo>, runs: Run[] = [makeRun()]) {
    project.found = { repo: null, remote: null, branch: null, loading: false, ...found };
    api.runs.mockResolvedValue({ runs, total: runs.length, nextPage: null });
    const view = render(<Glyph projectCwd="/code/sikemux" stripHovered={false} />);
    await act(async () => {});
    return view;
}

beforeEach(() => {
    invalidate(() => true);
    api.status.mockReset().mockResolvedValue({ ok: true });
    api.runs.mockReset();
    commands.openGitArea.mockClear();
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("the top bar's CI glyph", () => {
    it("shows how the branch's latest run went and opens the Actions section on click", async () => {
        await renderGlyph({ repo, branch: "feat/x" });
        expect(api.status).toHaveBeenCalledWith("work");
        expect(api.runs).toHaveBeenCalledWith({ ...repo, branch: "feat/x", perPage: 1 });
        const glyph = screen.getByRole("button", { name: "Failed · CI #12 · 5m ago" });
        expect(glyph.textContent).toBe("#12");
        fireEvent.click(glyph);
        expect(commands.openGitArea).toHaveBeenCalledWith("actions");
    });

    it("asks for the latest run on any branch when the project's branch is unknown", async () => {
        await renderGlyph({ repo });
        expect(api.runs).toHaveBeenCalledWith({ ...repo, branch: undefined, perPage: 1 });
    });

    it("shows nothing for a project that is not on this host", async () => {
        const view = await renderGlyph({ repo: { ...repo, provider: "elsewhere" } });
        expect(api.status).not.toHaveBeenCalled();
        expect(view.container.textContent).toBe("");
    });

    it("shows nothing for a project with no repository", async () => {
        const view = await renderGlyph({ repo: null });
        expect(view.container.textContent).toBe("");
    });

    it("asks for no runs until someone is signed in", async () => {
        api.status.mockResolvedValue({ ok: false });
        const view = await renderGlyph({ repo });
        expect(api.runs).not.toHaveBeenCalled();
        expect(view.container.textContent).toBe("");
    });

    it("shows nothing on a branch with no runs", async () => {
        const view = await renderGlyph({ repo }, []);
        expect(view.container.textContent).toBe("");
    });

    it("reads again every ten seconds while the run is going, and every twenty once it is not", async () => {
        vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
        await renderGlyph({ repo }, [makeRun({ status: "in_progress", conclusion: null })]);
        expect(screen.getByRole("button", { name: /^Running · / })).toBeTruthy();
        api.runs.mockResolvedValue({ runs: [makeRun({ conclusion: "success" })], total: 1, nextPage: null });
        await act(async () => {
            vi.advanceTimersByTime(10_000);
        });
        expect(api.runs).toHaveBeenCalledTimes(2);
        expect(screen.getByRole("button", { name: /^Passed · / })).toBeTruthy();
        await act(async () => {
            vi.advanceTimersByTime(10_000);
        });
        expect(api.runs).toHaveBeenCalledTimes(2);
        await act(async () => {
            vi.advanceTimersByTime(10_000);
        });
        expect(api.runs).toHaveBeenCalledTimes(3);
    });
});
