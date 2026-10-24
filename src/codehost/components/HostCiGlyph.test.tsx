import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Run } from "../api";
import type { HostRepo } from "../project";

const api = vi.hoisted(() => ({ status: vi.fn(), runs: vi.fn() }));
const project = vi.hoisted(() => ({ found: { repo: null, remote: null, branch: null, loading: false } as HostRepo }));

vi.mock("../project", async (importOriginal) => ({ ...(await importOriginal<object>()), useHostRepo: () => project.found }));

import { invalidate } from "../../plugin-api/resources";
import { ciCore, registerTestHost, TEST_HOST } from "../testHost";
import { hostCiGlyph } from "./HostCiGlyph";

registerTestHost(api);
const core = ciCore(api);
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
    outcome: "failure",
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
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("the top bar's CI glyph", () => {
    it("shows nothing for a project that is not on this host", async () => {
        const view = await renderGlyph({ repo: { ...repo, provider: "elsewhere" } });
        expect(api.status).not.toHaveBeenCalled();
        expect(view.container.textContent).toBe("");
    });

    it("asks for no runs until someone is signed in", async () => {
        api.status.mockResolvedValue({ ok: false });
        const view = await renderGlyph({ repo });
        expect(api.runs).not.toHaveBeenCalled();
        expect(view.container.textContent).toBe("");
    });

    it("has the core check the repository's CI, and reads again when the core says the runs moved", async () => {
        await renderGlyph({ repo }, [makeRun({ status: "in_progress", conclusion: null, outcome: "running" })]);
        expect(screen.getByRole("button", { name: /^Running · / })).toBeTruthy();
        expect(core.shown.at(-1)).toEqual({ repos: [expect.objectContaining({ provider: TEST_HOST, owner: "nodelike", name: "sikemux" })] });
        api.runs.mockResolvedValue({ runs: [makeRun({ conclusion: "success", outcome: "success" })], total: 1, nextPage: null });
        await act(async () => {
            core.listsMoved();
        });
        expect(api.runs).toHaveBeenCalledTimes(2);
        expect(screen.getByRole("button", { name: /^Passed · / })).toBeTruthy();
    });
});
