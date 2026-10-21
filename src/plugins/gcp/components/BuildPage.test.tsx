import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const host = vi.hoisted(() => ({ confirm: vi.fn(() => Promise.resolve(true)), prompt: vi.fn(() => Promise.resolve("release")) }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    confirmDialog: host.confirm,
    promptDialog: host.prompt,
}));

import { answerEmpty } from "../testing";
import { BuildPage, buildForAgent } from "./BuildPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const failed = {
    id: "b1f2c3d4-0000",
    status: "FAILURE",
    detail: "Build step failure",
    failure: "step deploy exited with 2",
    trigger: "deploy-main",
    triggerId: "t1",
    repo: "acme/storefront",
    branch: "main",
    commit: "abc1234def",
    steps: [
        { id: "test", image: "node:20", status: "SUCCESS", startedAt: "2024-10-18T11:00:00Z", endedAt: "2024-10-18T11:01:00Z" },
        {
            id: "deploy",
            image: "gcr.io/cloud-builders/gcloud",
            status: "FAILURE",
            startedAt: "2024-10-18T11:01:00Z",
            endedAt: "2024-10-18T11:01:30Z",
        },
    ],
    images: null,
    createdAt: "2024-10-18T11:00:00Z",
    startedAt: "2024-10-18T11:00:00Z",
    finishedAt: "2024-10-18T11:01:30Z",
    logUrl: null,
    region: null,
};
const detail = { ...failed, log: ['Step #0 - "test": ok', 'Step #1 - "deploy": ERROR: permission denied'], logSkipped: 0 };

let pane = 0;
const renderPage = () => render(<BuildPage paneId={`cb-${++pane}`} active target={target} />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.builds.mockResolvedValue([failed]);
    api.build.mockResolvedValue(detail);
    api.buildChange.mockResolvedValue({ ...failed, id: "retried-1", status: "QUEUED" });
    api.triggers.mockResolvedValue([
        {
            id: "t1",
            name: "deploy-main",
            description: null,
            repo: "acme/storefront",
            branch: "^main$",
            pullRequests: false,
            file: "cloudbuild.yaml",
            disabled: false,
        },
    ]);
    api.triggerRun.mockResolvedValue({ ...failed, id: "run-2", status: "QUEUED" });
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Cloud Build", () => {
    it("hands an agent the failed step and the end of the log", () => {
        const text = buildForAgent(detail);
        expect(text).toContain("Cloud Build b1f2c3d4-0000: FAILURE (trigger deploy-main)");
        expect(text).toContain("Failed step: deploy");
        expect(text).toContain("ERROR: permission denied");
        expect(text).toContain("gcp_get with kind build");
    });

    it("opens a failed build with its steps and log, and retries it", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /b1f2c3d4/ }));
        expect(await screen.findByText("step deploy exited with 2")).toBeInTheDocument();
        expect(screen.getByText(/ERROR: permission denied/)).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Retry" }));
        await waitFor(() => expect(api.buildChange).toHaveBeenCalledWith(target, "b1f2c3d4-0000", null, "retry"));
        await waitFor(() => expect(api.build).toHaveBeenLastCalledWith(target, "retried-1", null));
    });

    it("runs a trigger on the branch the person names", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("tab", { name: "Triggers" }));
        fireEvent.click(await screen.findByRole("button", { name: "Run" }));
        await waitFor(() => expect(api.triggerRun).toHaveBeenCalledWith(target, "t1", "release"));
        expect(host.prompt).toHaveBeenCalledWith(expect.objectContaining({ initial: "main" }));
    });

    it("reads a running build again until it finishes", async () => {
        vi.useFakeTimers({ shouldAdvanceTime: true });
        try {
            api.build
                .mockResolvedValueOnce({ ...detail, status: "WORKING", log: ["Step #0: installing"] })
                .mockResolvedValue({ ...detail, status: "SUCCESS", log: ["Step #0: installing", "DONE"] });
            renderPage();
            fireEvent.click(await screen.findByRole("row", { name: /b1f2c3d4/ }));
            expect(await screen.findByText(/installing/)).toBeInTheDocument();
            await act(async () => {
                await vi.advanceTimersByTimeAsync(4500);
            });
            expect(await screen.findByText(/DONE/)).toBeInTheDocument();
        } finally {
            vi.useRealTimers();
        }
    });
});
