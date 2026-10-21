import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { STOREFRONT, STOREFRONT_DETAIL } from "../testing";

const api = vi.hoisted(() => ({ helmRelease: vi.fn(), helmRollback: vi.fn() }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), kubeApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { kubernetes } from "../resources";
import { canRollBackTo, HelmReleases, HelmReleaseView, helmTone } from "./Helm";

const scope = { context: "kind-dev", namespace: "helm-demo" };

afterEach(() => {
    cleanup();
    kubernetes.refresh();
});
beforeEach(() => {
    Object.values({ ...api, ...host }).forEach((mock) => mock.mockReset());
    api.helmRelease.mockResolvedValue(STOREFRONT_DETAIL);
    api.helmRollback.mockResolvedValue({ message: "Rollback was a success! Happy Helming!" });
    host.confirmDialog.mockResolvedValue(true);
});

describe("Helm releases", () => {
    it("colours a release by its status and lists its chart and revision", () => {
        expect(helmTone("deployed")).toBe("live");
        expect(helmTone("failed")).toBe("danger");
        expect(helmTone("pending-upgrade")).toBe("warn");
        expect(helmTone("superseded")).toBe("idle");
        const onOpen = vi.fn();
        render(<HelmReleases releases={[STOREFRONT]} find="" showNamespace onOpen={onOpen} />);
        const row = screen.getByRole("row", { name: "storefront" });
        expect(row).toHaveTextContent("demo-chart 0.1.0");
        expect(row).toHaveTextContent("helm-demo");
        expect(within(row).getByText("failed")).toBeInTheDocument();
        fireEvent.click(row);
        expect(onOpen).toHaveBeenCalledWith(STOREFRONT);
    });

    it("rolls back only to an earlier revision that once ran", () => {
        const [failed, deployed, first] = STOREFRONT_DETAIL.history;
        expect(canRollBackTo(failed, 3)).toBe(false);
        expect(canRollBackTo(deployed, 3)).toBe(true);
        expect(canRollBackTo(first, 3)).toBe(true);
        expect(canRollBackTo({ ...first, status: "failed" }, 3)).toBe(false);
    });

    it("shows a release's history with why an upgrade failed, and what it made", async () => {
        render(<HelmReleaseView scope={scope} name="storefront" active onBack={vi.fn()} />);
        const history = await screen.findByRole("list", { name: "Release history" });
        expect(within(history).getAllByRole("listitem")).toHaveLength(3);
        expect(within(history).getByText(/context deadline exceeded/)).toBeInTheDocument();
        expect(within(screen.getByRole("listitem", { name: "Revision 3" })).queryByRole("button")).toBeNull();
        expect(screen.getByRole("list", { name: "What it made" })).toHaveTextContent("Deploymentstorefront-demo-chart");
        expect(screen.getByText(/Values and chart notes aren't shown/)).toBeInTheDocument();
        expect(api.helmRelease).toHaveBeenCalledWith(scope, "storefront");
    });

    it("rolls a release back once the person agrees, showing the helm command it runs", async () => {
        render(<HelmReleaseView scope={scope} name="storefront" active onBack={vi.fn()} />);
        fireEvent.click(await screen.findByRole("button", { name: "Roll back to revision 2" }));
        await waitFor(() => expect(api.helmRollback).toHaveBeenCalledWith(scope, "storefront", 2));
        const asked = host.confirmDialog.mock.calls[0][0];
        expect(asked.body).toBe("Helm makes it revision 4, with revision 2's chart and values.");
        expect(asked.commands[0].command).toBe("helm rollback storefront 2 --kube-context=kind-dev --namespace=helm-demo");
        expect(host.notify).toHaveBeenCalledWith("success", "Rollback was a success! Happy Helming!");
    });
});
