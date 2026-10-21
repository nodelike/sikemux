import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});

import { answerEmpty } from "../testing";
import { gcpSettings, getGcpView } from "../state";
import { GcpPane, goBack } from "./GcpPane";
import { search } from "./Search";
import { gcp } from "../resources";

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    gcpSettings.update(() => ({ account: null, project: "acme-prod", sideWidth: null }));
    api.runServices.mockResolvedValue([
        { name: "storefront", location: "us-central1", state: "READY", traffic: [], url: null, updatedAt: null, updatedBy: null },
    ]);
    api.runService.mockResolvedValue({
        name: "storefront",
        location: "us-central1",
        state: "READY",
        traffic: [],
        url: null,
        updatedAt: null,
        updatedBy: null,
        revisions: [],
        env: [],
    });
    api.instances.mockResolvedValue([
        { name: "store-db-vm", zone: "us-central1-a", status: "RUNNING", internalIps: [], externalIps: [], disks: [], spot: false },
    ]);
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("search", () => {
    it("matches products and resources, names that start with the words first", () => {
        const hits = search("sto", [
            { product: "compute", open: "z/store-db-vm", label: "store-db-vm", detail: "VM instance" },
            { product: "run", open: "r/restore", label: "restore", detail: "Cloud Run service" },
        ]);
        expect(hits.map((hit) => hit.label)).toEqual(["store-db-vm", "Cloud Storage", "restore"]);
        expect(search("  ", [])).toEqual([]);
    });

    it("opens a resource found from the top bar, and Esc climbs back out", async () => {
        render(<GcpPane paneId="search-1" active />);
        const box = await screen.findByRole("textbox", { name: "Search Google Cloud" });
        fireEvent.focus(box);
        fireEvent.change(box, { target: { value: "storefront" } });
        fireEvent.click(await screen.findByRole("option", { name: /storefront/ }));
        await waitFor(() => expect(getGcpView("search-1")).toMatchObject({ product: "run", open: "us-central1/storefront", list: "services" }));
        expect(goBack("search-1")).toBe(true);
        expect(getGcpView("search-1")).toMatchObject({ product: "run", open: null });
        expect(goBack("search-1")).toBe(true);
        expect(getGcpView("search-1").product).toBe("overview");
        expect(goBack("search-1")).toBe(false);
    });
});
