import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WEB_HISTORY } from "../testing";

const api = vi.hoisted(() => ({ history: vi.fn(), act: vi.fn(), yaml: vi.fn() }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn(), copyWithNotice: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), kubeApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { kubernetes } from "../resources";
import { imageChange, WorkloadView } from "./WorkloadView";

const scope = { context: "kind-dev", namespace: "shop" };
const renderView = () => render(<WorkloadView scope={scope} kind="Deployment" name="web" active onBack={vi.fn()} />);

afterEach(() => {
    cleanup();
    kubernetes.refresh();
});
beforeEach(() => {
    Object.values({ ...api, ...host }).forEach((mock) => mock.mockReset());
    api.history.mockResolvedValue(WEB_HISTORY);
    api.act.mockResolvedValue({ message: "deployment.apps/web rolled back" });
    api.yaml.mockResolvedValue("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n");
    host.confirmDialog.mockResolvedValue(true);
});

describe("a workload's page", () => {
    it("lists its revisions newest first, with the live one, restarts and why each was made", async () => {
        renderView();
        const revisions = await screen.findByRole("list", { name: "Revisions" });
        const items = within(revisions).getAllByRole("listitem");
        expect(items).toHaveLength(4);
        expect(within(items[0]).getByText("Live")).toBeInTheDocument();
        expect(within(items[0]).queryByRole("button")).toBeNull();
        expect(within(items[1]).getByText("move to nginx 1.27")).toBeInTheDocument();
        expect(within(items[2]).getByText("Restart")).toBeInTheDocument();
        expect(api.history).toHaveBeenCalledWith(scope, "Deployment", "web");
    });

    it("rolls back to a revision once the person agrees, warning that kubectl apply would undo it", async () => {
        renderView();
        fireEvent.click(await screen.findByRole("button", { name: "Roll back to revision 3" }));
        await waitFor(() => expect(api.act).toHaveBeenCalledWith(scope, "undo", "web", "Deployment", undefined, 3));
        const asked = host.confirmDialog.mock.calls[0][0];
        expect(asked.body).toContain("nginx goes from 1.27.3-alpine to 1.27-alpine.");
        expect(asked.body).toContain("kubectl apply");
        expect(asked.commands[0].command).toBe("kubectl --context kind-dev --namespace shop rollout undo deployment/web --to-revision=3");
        expect(host.notify).toHaveBeenCalledWith("success", "deployment.apps/web rolled back");
    });

    it("leaves a rollout alone when the person says no", async () => {
        host.confirmDialog.mockResolvedValue(false);
        renderView();
        fireEvent.click(await screen.findByRole("button", { name: "Roll back to revision 1" }));
        await waitFor(() => expect(host.confirmDialog).toHaveBeenCalled());
        expect(api.act).not.toHaveBeenCalled();
    });

    it("pauses a Deployment's rollout, and offers to resume one already paused", async () => {
        renderView();
        fireEvent.click(await screen.findByRole("button", { name: "Pause rollout" }));
        await waitFor(() => expect(api.act).toHaveBeenCalledWith(scope, "pauseRollout", "web", "Deployment"));
        cleanup();
        kubernetes.refresh();
        api.history.mockResolvedValue({ ...WEB_HISTORY, paused: true });
        renderView();
        expect(await screen.findByText("Rollout paused")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Resume rollout" })).toBeInTheDocument();
    });

    it("shows its YAML, numbered, with a copy button", async () => {
        renderView();
        fireEvent.click(within(await screen.findByRole("group", { name: "Workload view" })).getByRole("button", { name: "YAML" }));
        const yaml = await screen.findByRole("region", { name: "Deployment web YAML" });
        expect(yaml).toHaveTextContent("kind: Deployment");
        expect(api.yaml).toHaveBeenCalledWith(scope, "Deployment", "web");
        fireEvent.click(within(yaml).getByRole("button", { name: "Copy" }));
        expect(host.copyWithNotice).toHaveBeenCalledWith("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n", "The YAML");
    });
});

describe("what an undo changes", () => {
    it("names the images it goes back to, or says they stay the same", () => {
        const [live, , restart, first] = WEB_HISTORY.revisions;
        expect(imageChange(live, first)).toBe("nginx goes from 1.27.3-alpine to alpine.");
        expect(imageChange({ ...live, images: ["redis:7"] }, first)).toBe("Images go from redis:7 to nginx:alpine.");
        expect(
            imageChange(
                { ...live, images: ["europe-docker.pkg.dev/acme/shop/api:4.13.0"] },
                { ...first, images: ["europe-docker.pkg.dev/acme/shop/api"] },
            ),
        ).toBe("api goes from 4.13.0 to latest.");
        expect(imageChange(restart, first)).toMatch(/same as now/);
    });
});
