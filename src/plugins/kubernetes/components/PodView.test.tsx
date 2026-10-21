import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WORKER_DETAIL } from "../testing";

const api = vi.hoisted(() => ({ pod: vi.fn(), logs: vi.fn(), followLogs: vi.fn(), act: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), kubeApi: api }));
vi.mock("../../../plugin-api/ui", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    VirtualLogList: ({ items, renderRow }: { items: unknown[]; renderRow: (item: unknown, index: number) => unknown }) => (
        <div>{items.map((item, index) => renderRow(item, index) as never)}</div>
    ),
}));

import { kubernetes } from "../resources";
import { levelOf } from "./PodLogs";
import { containerTone, kubectlScope, PodView, stateLine } from "./PodView";

const scope = { context: "kind-dev", namespace: "shop" };
const renderView = (onBack = vi.fn()) => render(<PodView scope={scope} name={WORKER_DETAIL.name} active onBack={onBack} />);

afterEach(() => {
    cleanup();
    kubernetes.refresh();
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.pod.mockResolvedValue(WORKER_DETAIL);
    api.logs.mockResolvedValue({ lines: [{ at: "2026-10-19T08:39:59Z", text: "panic: queue unreachable" }], filtered: 0 });
    api.followLogs.mockImplementation((_scope, _pod, _container, handlers: { onItem: (batch: unknown) => void }) => {
        handlers.onItem({ lines: [{ at: "2026-10-19T08:41:02Z", text: "starting worker" }], ended: false });
        return { stop: vi.fn() };
    });
});

describe("a pod's page", () => {
    it("says why a crashing container is down and what it last exited with", async () => {
        renderView();
        const card = await screen.findByRole("article", { name: "Container worker" });
        expect(within(card).getByText("CrashLoopBackOff")).toBeInTheDocument();
        expect(within(card).getByText(/back-off 2m40s/)).toBeInTheDocument();
        expect(within(card).getByText(/Last run ended: Error \(code 1\)/)).toBeInTheDocument();
        expect(within(card).getByText("busybox:1.36")).toBeInTheDocument();
        expect(within(card).getByText("secret worker-secrets")).toBeInTheDocument();
        expect(screen.getByRole("list", { name: "Events" })).toHaveTextContent("×14");
        expect(screen.getByRole("button", { name: "Shell" })).toBeDisabled();
    });

    it("follows the logs live, and reads the run before the restart on its own", async () => {
        renderView();
        fireEvent.click(await screen.findByRole("button", { name: "Logs" }));
        expect(await screen.findByText("starting worker")).toBeInTheDocument();
        expect(api.followLogs).toHaveBeenCalledWith(scope, WORKER_DETAIL.name, "worker", expect.anything());
        fireEvent.click(within(screen.getByText("Before the last restart").closest("label") as HTMLElement).getByRole("switch"));
        expect(await screen.findByText("panic: queue unreachable")).toBeInTheDocument();
        expect(api.logs).toHaveBeenCalledWith(scope, WORKER_DETAIL.name, "worker", true);
    });

    it("says when the pod has gone", async () => {
        api.pod.mockResolvedValue(null);
        renderView();
        expect(await screen.findByText(/This pod is gone/)).toBeInTheDocument();
    });

    it("goes back to the list", async () => {
        const onBack = vi.fn();
        renderView(onBack);
        fireEvent.click(await screen.findByRole("button", { name: "Back to pods" }));
        await waitFor(() => expect(onBack).toHaveBeenCalled());
    });
});

describe("container words", () => {
    it("reads a container's state as a person would say it", () => {
        const [worker] = WORKER_DETAIL.containerStates;
        expect(stateLine(worker)).toBe("CrashLoopBackOff");
        expect(containerTone(worker)).toBe("danger");
        expect(stateLine({ ...worker, state: "terminated", reason: "Completed", exitCode: 0 })).toBe("Completed with code 0");
        expect(containerTone({ ...worker, state: "terminated", exitCode: 0 })).toBe("idle");
        expect(containerTone({ ...worker, state: "waiting", reason: "ContainerCreating" })).toBe("warn");
    });

    it("names the cluster and namespace for kubectl", () => {
        expect(kubectlScope(scope)).toEqual(["--context", "kind-dev", "--namespace", "shop"]);
        expect(kubectlScope({})).toEqual([]);
    });
});

describe("log levels", () => {
    it("picks out error and warning lines", () => {
        expect(levelOf('level=error msg="capture failed"')).toBe("error");
        expect(levelOf("panic: capture failed")).toBe("error");
        expect(levelOf("2026-10-19 WARN slow query")).toBe("warn");
        expect(levelOf("level=info msg=connected")).toBeNull();
    });
});
