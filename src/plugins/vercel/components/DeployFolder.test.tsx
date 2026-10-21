import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginStreamHandlers } from "../../../plugin-api/backend";
import type { DeployProgress, VercelDeployment, VercelProject } from "../api";

const streams = vi.hoisted(() => ({ handlers: [] as PluginStreamHandlers<DeployProgress>[], stopped: 0 }));
const api = vi.hoisted(() => ({
    deployFolder: vi.fn((_team: string, _project: string, _folder: string, _production: boolean, handlers: PluginStreamHandlers<DeployProgress>) => {
        streams.handlers.push(handlers);
        return { stop: () => (streams.stopped += 1) };
    }),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));
const host = vi.hoisted(() => ({
    confirmDialog: vi.fn(),
    pickFolder: vi.fn(),
    useNearestProject: vi.fn(() => ({ name: "storefront", cwd: "/Users/me/storefront" })),
}));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { DeployFolder, progressLabel } from "./DeployFolder";

const project = { id: "prj_store", name: "storefront" } as VercelProject;

beforeEach(() => {
    streams.handlers = [];
    streams.stopped = 0;
    api.deployFolder.mockClear();
    host.confirmDialog.mockReset().mockResolvedValue(true);
    host.pickFolder.mockReset();
});
afterEach(cleanup);

describe("deploying a folder", () => {
    it("says how far the upload got", () => {
        expect(progressLabel({ step: "Uploading", done: 3, total: 12, deployment: null })).toBe("Uploading 3 of 12 files");
        expect(progressLabel({ step: "Reading files", done: 0, total: 0, deployment: null })).toBe("Reading files");
        expect(progressLabel(null)).toBe("");
    });

    it("deploys the open project's folder as a preview, and opens the deployment Vercel builds", () => {
        const onDeployed = vi.fn();
        render(<DeployFolder team="team_acme" project={project} onDeployed={onDeployed} onClose={vi.fn()} />);
        expect(screen.getByText("/Users/me/storefront")).toBeInTheDocument();
        expect(screen.getByRole("status")).toHaveTextContent("every .env file, stay on this Mac");
        fireEvent.click(screen.getByRole("button", { name: "Deploy" }));
        return waitFor(() => expect(api.deployFolder).toHaveBeenCalled()).then(() => {
            expect(api.deployFolder).toHaveBeenCalledWith("team_acme", "prj_store", "/Users/me/storefront", false, expect.anything());
            expect(host.confirmDialog).not.toHaveBeenCalled();
            act(() => streams.handlers[0]!.onItem({ step: "Uploading", done: 1, total: 4, deployment: null }));
            expect(screen.getByRole("status")).toHaveTextContent("Uploading 1 of 4 files");
            act(() => streams.handlers[0]!.onItem({ step: "Building", done: 4, total: 4, deployment: { id: "dpl_new" } as VercelDeployment }));
            expect(onDeployed).toHaveBeenCalledWith("dpl_new");
        });
    });

    it("asks before deploying to production, and does nothing when told no", async () => {
        host.confirmDialog.mockResolvedValue(false);
        render(<DeployFolder team="team_acme" project={project} onDeployed={vi.fn()} onClose={vi.fn()} />);
        fireEvent.click(screen.getByRole("button", { name: "Production" }));
        fireEvent.click(screen.getByRole("button", { name: "Deploy" }));
        await waitFor(() => expect(host.confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ title: "Deploy storefront to production?" })));
        expect(api.deployFolder).not.toHaveBeenCalled();
    });

    it("shows why Vercel or git refused", async () => {
        render(<DeployFolder team="team_acme" project={project} onDeployed={vi.fn()} onClose={vi.fn()} />);
        fireEvent.click(screen.getByRole("button", { name: "Deploy" }));
        await waitFor(() => expect(api.deployFolder).toHaveBeenCalled());
        act(() => streams.handlers[0]!.onError!({ category: "bad-params", message: "invalid argument: /Users/me/storefront is not a git checkout" }));
        expect(screen.getByRole("status")).toHaveTextContent("is not a git checkout");
        expect(screen.getByRole("button", { name: "Deploy" })).toBeEnabled();
    });
});
