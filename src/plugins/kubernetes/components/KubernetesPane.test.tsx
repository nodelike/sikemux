import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NIGHTLY, noKubectl, ready, WEB, WEB_POD, WORKER, WORKER_POD, CONTROL, SICK, WEB_HISTORY, STOREFRONT } from "../testing";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    namespaces: vi.fn(),
    workloads: vi.fn(),
    pods: vi.fn(),
    events: vi.fn(),
    network: vi.fn(),
    config: vi.fn(),
    pod: vi.fn(),
    nodes: vi.fn(),
    history: vi.fn(),
    helmReleases: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), kubeApi: api }));

import { kubernetes } from "../resources";
import { KubernetesPane } from "./KubernetesPane";

let pane = 0;
const renderPane = () => render(<KubernetesPane paneId={`k8s-${++pane}`} active />);

afterEach(() => {
    cleanup();
    kubernetes.refresh();
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.status.mockResolvedValue(ready);
    api.namespaces.mockResolvedValue([
        { name: "default", phase: "Active" },
        { name: "shop", phase: "Active" },
    ]);
    api.workloads.mockResolvedValue([WORKER, WEB, NIGHTLY]);
    api.pods.mockResolvedValue([]);
    api.events.mockResolvedValue([]);
    api.network.mockResolvedValue({ services: [], ingresses: [] });
    api.config.mockResolvedValue({ configMaps: [], secrets: [] });
    api.nodes.mockResolvedValue([CONTROL]);
    api.history.mockResolvedValue(WEB_HISTORY);
    api.helmReleases.mockResolvedValue([STOREFRONT]);
});

const openSection = async (name: RegExp) =>
    fireEvent.click(within(await screen.findByRole("navigation", { name: "Kubernetes" })).getByRole("button", { name }));

describe("the Kubernetes pane", () => {
    it("says what is missing before there is a cluster to show", async () => {
        api.status.mockResolvedValue(noKubectl);
        renderPane();
        expect(await screen.findByText("kubectl isn't installed")).toBeInTheDocument();
        expect(screen.getByText(/brew install kubernetes-cli/)).toBeInTheDocument();
    });

    it("lists the current cluster's workloads, flagging the ones in trouble", async () => {
        renderPane();
        const table = await screen.findByRole("table", { name: "Workloads" });
        expect(within(table).getAllByRole("row")).toHaveLength(4);
        expect(within(table).getByText("None of 1 ready")).toBeInTheDocument();
        expect(within(table).getByText(/0 2 \* \* \*/)).toBeInTheDocument();
        expect(api.workloads).toHaveBeenCalledWith({ context: "kind-dev", namespace: "default" }, false);
        const nav = screen.getByRole("navigation", { name: "Kubernetes" });
        expect(within(nav).getByRole("button", { name: /Workloads/ })).toHaveTextContent("1");
        fireEvent.click(within(screen.getByRole("group", { name: "Show" })).getByRole("button", { name: /^Needs attention/ }));
        expect(within(table).getAllByRole("row")).toHaveLength(2);
    });

    it("reads every namespace at once when asked", async () => {
        renderPane();
        await screen.findByRole("table", { name: "Workloads" });
        fireEvent.click(screen.getByRole("button", { name: /Namespace/ }));
        fireEvent.click(await screen.findByRole("option", { name: /All namespaces/ }));
        await waitFor(() => expect(api.workloads).toHaveBeenLastCalledWith({ context: "kind-dev", namespace: undefined }, true));
    });

    it("opens a workload's own pods, worst first, and lets the narrowing go", async () => {
        api.pods.mockResolvedValue([WORKER_POD, WEB_POD]);
        renderPane();
        const workloads = await screen.findByRole("table", { name: "Workloads" });
        fireEvent.click(within(workloads).getByRole("row", { name: "Deployment worker" }));
        const pods = await screen.findByRole("table", { name: "Pods" });
        expect(api.pods).toHaveBeenLastCalledWith({ context: "kind-dev", namespace: "default" }, false, "app=worker");
        expect(within(pods).getByText("CrashLoopBackOff")).toBeInTheDocument();
        expect(within(pods).getByText("6")).toHaveClass("k8s-restarts");
        fireEvent.click(screen.getByRole("button", { name: /app=worker/ }));
        await waitFor(() => expect(api.pods).toHaveBeenLastCalledWith({ context: "kind-dev", namespace: "default" }, false, null));
    });

    it("finds pods by name, owner or node", async () => {
        api.pods.mockResolvedValue([WORKER_POD, WEB_POD]);
        renderPane();
        fireEvent.click(within(await screen.findByRole("navigation", { name: "Kubernetes" })).getByRole("button", { name: /Pods/ }));
        const pods = await screen.findByRole("table", { name: "Pods" });
        fireEvent.change(screen.getByRole("textbox", { name: "Find" }), { target: { value: "web" } });
        expect(within(pods).getAllByRole("row")).toHaveLength(2);
        fireEvent.click(within(screen.getByRole("group", { name: "Show" })).getByRole("button", { name: /^Needs attention/ }));
        expect(screen.getByText("No pods match.")).toBeInTheDocument();
    });

    it("shows services with the pods they pick, and where ingresses route", async () => {
        api.network.mockResolvedValue({
            services: [
                {
                    name: "web",
                    namespace: "shop",
                    kind: "ClusterIP",
                    clusterIp: "10.96.4.2",
                    external: [],
                    ports: ["80→8080/TCP"],
                    selector: "app=web",
                },
            ],
            ingresses: [
                {
                    name: "shop",
                    namespace: "shop",
                    class: "nginx",
                    tls: ["shop.acme.dev"],
                    address: [],
                    rules: [{ host: "shop.acme.dev", path: "/", service: "web", port: "80" }],
                },
            ],
        });
        renderPane();
        await openSection(/Services/);
        const services = await screen.findByRole("table", { name: "Services" });
        expect(within(services).getByText("80→8080/TCP")).toBeInTheDocument();
        expect(screen.getByRole("article", { name: "Ingress shop" })).toHaveTextContent("web:80");
        expect(screen.getByRole("button", { name: "shop.acme.dev/" })).toHaveAttribute("title", "Open https://shop.acme.dev/");
        fireEvent.click(within(services).getByRole("button", { name: "app=web" }));
        await waitFor(() => expect(api.pods).toHaveBeenLastCalledWith({ context: "kind-dev", namespace: "default" }, false, "app=web"));
    });

    it("lists config by key names only", async () => {
        api.config.mockResolvedValue({
            configMaps: [{ name: "web-config", namespace: "shop", kind: null, keys: [{ name: "API_URL", size: 21 }] }],
            secrets: [{ name: "web-secrets", namespace: "shop", kind: "Opaque", keys: [{ name: "DATABASE_PASSWORD", size: 2048 }] }],
        });
        renderPane();
        await openSection(/Config/);
        expect(await screen.findByRole("article", { name: "web-config" })).toHaveTextContent("21 B");
        fireEvent.click(within(screen.getByRole("group", { name: "Kind" })).getByRole("button", { name: /^Secrets/ }));
        expect(screen.getByRole("article", { name: "web-secrets" })).toHaveTextContent("DATABASE_PASSWORD2.0 KB");
    });

    it("counts warnings in the sidebar and shows them first", async () => {
        api.events.mockResolvedValue([
            {
                kind: "Warning",
                reason: "BackOff",
                message: "Back-off restarting failed container",
                count: 9,
                last: null,
                object: "Pod/worker-1",
                namespace: "shop",
            },
            { kind: "Normal", reason: "Scheduled", message: "Assigned to node", count: 1, last: null, object: "Pod/web-1", namespace: "shop" },
        ]);
        renderPane();
        await openSection(/Events/);
        const list = await screen.findByRole("list", { name: "Events" });
        expect(within(list).getAllByRole("listitem")).toHaveLength(1);
        expect(within(screen.getByRole("navigation", { name: "Kubernetes" })).getByRole("button", { name: /Events/ })).toHaveTextContent("1");
        fireEvent.click(within(screen.getByRole("group", { name: "Show" })).getByRole("button", { name: /^All/ }));
        expect(within(list).getAllByRole("listitem")).toHaveLength(2);
        api.pod.mockResolvedValue(null);
        fireEvent.click(within(list).getByRole("button", { name: "Pod/worker-1" }));
        await waitFor(() => expect(api.pod).toHaveBeenCalledWith({ context: "kind-dev", namespace: "shop" }, "worker-1"));
    });

    it("reads the cluster again when Refresh is pressed", async () => {
        renderPane();
        await screen.findByRole("table", { name: "Workloads" });
        const before = api.workloads.mock.calls.length;
        fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
        await waitFor(() => expect(api.workloads.mock.calls.length).toBeGreaterThan(before));
    });

    it("lists the cluster's nodes, and counts the ones not ready in the sidebar", async () => {
        api.nodes.mockResolvedValue([SICK, CONTROL]);
        renderPane();
        const nav = await screen.findByRole("navigation", { name: "Kubernetes" });
        await waitFor(() => expect(within(nav).getByRole("button", { name: /Nodes/ })).toHaveTextContent("1"));
        fireEvent.click(within(nav).getByRole("button", { name: /Nodes/ }));
        const table = await screen.findByRole("table", { name: "Nodes" });
        expect(within(table).getAllByRole("row")).toHaveLength(3);
        expect(api.nodes).toHaveBeenCalledWith("kind-dev");
    });

    it("opens a workload's history from its row, and goes back to the list", async () => {
        renderPane();
        const table = await screen.findByRole("table", { name: "Workloads" });
        fireEvent.click(within(table).getByRole("button", { name: "History of web" }));
        expect(await screen.findByRole("list", { name: "Revisions" })).toBeInTheDocument();
        expect(api.history).toHaveBeenCalledWith({ context: "kind-dev", namespace: "shop" }, "Deployment", "web");
        fireEvent.click(screen.getByRole("button", { name: "Back to workloads" }));
        expect(await screen.findByRole("table", { name: "Workloads" })).toBeInTheDocument();
        expect(within(screen.getByRole("table", { name: "Workloads" })).queryByRole("button", { name: "History of nightly-report" })).toBeNull();
    });

    it("lists Helm releases and counts the failed ones in the sidebar", async () => {
        renderPane();
        const nav = await screen.findByRole("navigation", { name: "Kubernetes" });
        fireEvent.click(within(nav).getByRole("button", { name: /Helm/ }));
        const table = await screen.findByRole("table", { name: "Helm releases" });
        expect(within(table).getByRole("row", { name: "storefront" })).toBeInTheDocument();
        expect(api.helmReleases).toHaveBeenCalledWith({ context: "kind-dev", namespace: "default" }, false);
        await waitFor(() => expect(within(nav).getByRole("button", { name: /Helm/ })).toHaveTextContent("1"));
    });
});
