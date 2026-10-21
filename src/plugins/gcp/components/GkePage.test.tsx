import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const host = vi.hoisted(() => ({ confirm: vi.fn(() => Promise.resolve(true)), prompt: vi.fn(() => Promise.resolve("5")) }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    confirmDialog: host.confirm,
    promptDialog: host.prompt,
}));

import { answerEmpty } from "../testing";
import { GkePage } from "./GkePage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const cluster = {
    name: "prod",
    location: "us-central1",
    status: "RUNNING",
    message: null,
    version: "1.30.4-gke.1",
    nodeVersion: "1.30.4-gke.1",
    nodes: 3,
    autopilot: false,
    channel: "REGULAR",
    network: "default",
    private: false,
    nodePools: [
        {
            name: "default-pool",
            status: "RUNNING",
            version: "1.30",
            machineType: "e2-standard-4",
            diskSizeGb: 100,
            spot: false,
            nodes: 3,
            autoscaling: false,
            minNodes: null,
            maxNodes: null,
        },
    ],
    createdAt: null,
};
const api_ = {
    name: "api",
    namespace: "shop",
    kind: "Deployment",
    desired: 3,
    ready: 2,
    containers: [{ name: "api", image: "img:1" }],
    selector: "app=api",
    createdAt: null,
};
const crashing = {
    name: "api-7d9-x1",
    namespace: "shop",
    phase: "Running",
    reason: "CrashLoopBackOff",
    ready: "0/1",
    restarts: 7,
    node: "n1",
    ip: "10.4.0.9",
    containers: [{ name: "api", image: "img:1" }],
    createdAt: null,
};

let pane = 0;
const renderPage = () => render(<GkePage paneId={`gke-${++pane}`} active target={target} />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.clusters.mockResolvedValue([cluster]);
    api.workloads.mockResolvedValue([api_]);
    api.pods.mockResolvedValue([crashing]);
    api.kubeServices.mockResolvedValue([]);
    api.nodes.mockResolvedValue([
        {
            name: "gke-prod-default-pool-a1",
            ready: true,
            pressure: ["MemoryPressure"],
            unschedulable: false,
            pool: "default-pool",
            machineType: "e2-standard-4",
            zone: "us-central1-a",
            spot: false,
            cpu: "3920m",
            memory: "13Gi",
            pods: "110",
            version: "v1.30",
            createdAt: null,
        },
    ]);
    api.podEvents.mockResolvedValue([
        { type: "Warning", reason: "BackOff", message: "Back-off restarting failed container api", count: 31, lastAt: null, source: "kubelet" },
    ]);
    api.podLogs.mockResolvedValue({ lines: [{ at: "2024-10-18T11:00:00Z", text: "panic: db unreachable" }] });
    api.deploymentRestart.mockResolvedValue(api_);
    api.deploymentScale.mockResolvedValue({ replicas: 5 });
    api.nodePoolResize.mockResolvedValue({ operation: "op" });
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

const openCluster = async () => {
    renderPage();
    fireEvent.click(await screen.findByRole("row", { name: /prod/ }));
    expect(await screen.findByRole("heading", { name: "prod" })).toBeInTheDocument();
};

describe("Kubernetes Engine", () => {
    it("resizes a node pool to the count the person gives, beside the nodes themselves", async () => {
        await openCluster();
        expect(await screen.findByText("gke-prod-default-pool-a1")).toBeInTheDocument();
        fireEvent.click(within(await screen.findByRole("table", { name: "Node pools" })).getByRole("button", { name: "Resize" }));
        await waitFor(() => expect(api.nodePoolResize).toHaveBeenCalledWith(target, "us-central1", "prod", "default-pool", 5));
    });

    it("restarts and scales a Deployment", async () => {
        await openCluster();
        fireEvent.click(screen.getByRole("tab", { name: "Workloads" }));
        const table = await screen.findByRole("table", { name: "Workloads" });
        expect(within(table).getByText("2/3")).toBeInTheDocument();
        fireEvent.click(within(table).getByRole("button", { name: "Restart" }));
        await waitFor(() => expect(api.deploymentRestart).toHaveBeenCalledWith(target, "us-central1", "prod", "shop", "api"));
        fireEvent.click(within(table).getByRole("button", { name: "Scale" }));
        await waitFor(() => expect(api.deploymentScale).toHaveBeenCalledWith(target, "us-central1", "prod", "shop", "api", 5));
    });

    it("finds a workload's pods by its selector and reads a crashing pod's logs", async () => {
        await openCluster();
        fireEvent.click(screen.getByRole("tab", { name: "Workloads" }));
        fireEvent.click(await screen.findByRole("button", { name: "api" }));
        await waitFor(() => expect(api.pods).toHaveBeenCalledWith(target, "us-central1", "prod", "app=api"));
        const pods = await screen.findByRole("table", { name: "Pods" });
        expect(within(pods).getByText("CrashLoopBackOff")).toBeInTheDocument();
        fireEvent.click(within(pods).getByRole("row", { name: /api-7d9-x1/ }));
        expect(await screen.findByText(/panic: db unreachable/)).toBeInTheDocument();
        expect(await screen.findByText("Back-off restarting failed container api")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Previous run" }));
        await waitFor(() => expect(api.podLogs).toHaveBeenLastCalledWith(target, "us-central1", "prod", "shop", "api-7d9-x1", true));
    });
});
