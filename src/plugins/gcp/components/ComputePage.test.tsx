import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const host = vi.hoisted(() => ({ confirm: vi.fn(() => Promise.resolve(true)), terminal: vi.fn(() => Promise.resolve()) }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    confirmDialog: host.confirm,
    runInTerminal: host.terminal,
}));

import { answerEmpty } from "../testing";
import { ComputePage, powersFor } from "./ComputePage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const vm = {
    id: "123",
    name: "web-1",
    zone: "us-central1-a",
    machineType: "e2-medium",
    status: "RUNNING",
    internalIps: ["10.0.0.5"],
    externalIps: ["34.1.2.3"],
    disks: [{ name: "web-1", sizeGb: 20, boot: true, mode: "READ_WRITE" }],
    spot: true,
    serviceAccount: null,
    createdAt: null,
    startedAt: null,
};

let pane = 0;
const renderPage = () => render(<ComputePage paneId={`vm-${++pane}`} active target={target} />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.instances.mockResolvedValue([vm]);
    api.instance.mockResolvedValue({ ...vm, ssh: ["gcloud", "compute", "ssh", "web-1", "--zone=us-central1-a", "--project=acme-prod"] });
    api.instancePower.mockResolvedValue({ operation: "op" });
    api.instanceSerial.mockResolvedValue({ contents: "Booting...\nReached target Multi-User System.", start: 0, next: 100, trimmed: false });
    host.confirm.mockClear();
    host.terminal.mockClear();
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Compute Engine", () => {
    it("offers only the power changes a VM's state allows", () => {
        expect(powersFor("RUNNING")).toEqual(["stop", "reset", "suspend"]);
        expect(powersFor("TERMINATED")).toEqual(["start"]);
        expect(powersFor("SUSPENDED")).toEqual(["resume"]);
        expect(powersFor("STOPPING")).toEqual([]);
    });

    it("lists VMs with their zone, type and addresses", async () => {
        renderPage();
        const table = await screen.findByRole("table", { name: "VM instances" });
        expect(within(table).getByText("34.1.2.3")).toBeInTheDocument();
        expect(within(table).getByText("Spot")).toBeInTheDocument();
    });

    it("opens a VM, stops it once confirmed, and opens SSH in a terminal", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /web-1/ }));
        fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
        await waitFor(() => expect(api.instancePower).toHaveBeenCalledWith(target, "us-central1-a", "web-1", "stop"));
        expect(host.confirm).toHaveBeenCalledWith(expect.objectContaining({ destructive: true }));
        fireEvent.click(screen.getByRole("button", { name: "SSH" }));
        expect(host.terminal).toHaveBeenCalledWith("SSH web-1", ["gcloud", "compute", "ssh", "web-1", "--zone=us-central1-a", "--project=acme-prod"]);
    });

    it("shows what a VM printed to its serial console", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /web-1/ }));
        fireEvent.click(await screen.findByRole("tab", { name: "Serial console" }));
        expect(await screen.findByText(/Reached target Multi-User System/)).toBeInTheDocument();
    });

    it("takes a snapshot of a disk, and lists the project's snapshots", async () => {
        api.snapshotDisk.mockResolvedValue({ operation: "op", snapshot: "web-1-2024" });
        api.snapshots.mockResolvedValue([
            { name: "web-1-2024", disk: "web-1", status: "READY", sizeGb: 20, storageBytes: 1073741824, locations: ["us"], createdAt: null },
        ]);
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /web-1/ }));
        fireEvent.click(within(await screen.findByRole("table", { name: "Disks" })).getByRole("button", { name: "Snapshot" }));
        await waitFor(() => expect(api.snapshotDisk).toHaveBeenCalledWith(target, "us-central1-a", "web-1"));
        fireEvent.click(screen.getByRole("button", { name: "Back to VM instances" }));
        fireEvent.click(await screen.findByRole("tab", { name: "Snapshots" }));
        expect(await screen.findByText("web-1-2024")).toBeInTheDocument();
    });
});
