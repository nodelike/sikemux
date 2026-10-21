import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CONTROL, node, SICK } from "../testing";

const api = vi.hoisted(() => ({ cordon: vi.fn() }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), kubeApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { cores, memory } from "../format";
import { Nodes, nodeTone, statusLine } from "./Nodes";

afterEach(cleanup);
beforeEach(() => {
    Object.values({ ...api, ...host }).forEach((mock) => mock.mockReset());
    api.cordon.mockResolvedValue({ message: "node/kind-dev-control-plane cordoned" });
    host.confirmDialog.mockResolvedValue(true);
});

describe("nodes", () => {
    it("writes a node's status and size as kubectl and people say them", () => {
        expect(statusLine(SICK)).toBe("NotReady, cordoned");
        expect(statusLine({ status: "Ready", cordoned: true })).toBe("Cordoned");
        expect(statusLine(CONTROL)).toBe("Ready");
        expect(nodeTone(SICK)).toBe("danger");
        expect(nodeTone({ status: "Ready", pressure: ["DiskPressure"], cordoned: false })).toBe("warn");
        expect(nodeTone({ status: "Ready", pressure: [], cordoned: true })).toBe("idle");
        expect(cores(4000)).toBe("4");
        expect(cores(3800)).toBe("3.8");
        expect(cores(250)).toBe("250m");
        expect(memory(6_198_095_872)).toBe("5.8 GiB");
        expect(memory(512 * 1024 ** 2)).toBe("512 MiB");
    });

    it("shows why a node is down, its pressure and taints, and says use needs metrics-server", () => {
        render(<Nodes nodes={[SICK, CONTROL]} context="kind-dev" />);
        const sick = screen.getByRole("row", { name: "kind-dev-worker" });
        expect(within(sick).getByText("container runtime network not ready")).toBeInTheDocument();
        expect(within(sick).getByText("MemoryPressure")).toBeInTheDocument();
        expect(within(sick).getByText("dedicated=gpu:NoSchedule")).toBeInTheDocument();
        expect(screen.getByText(/metrics-server/)).toBeInTheDocument();
    });

    it("shows use against capacity once the cluster measures it", () => {
        render(<Nodes nodes={[node("busy", { cpuUsedMillis: 3600, memoryUsedBytes: 4 * 1024 ** 3 })]} context="kind-dev" />);
        expect(screen.getByRole("row", { name: "busy" })).toHaveTextContent("3.6 / 4");
        expect(screen.queryByText(/metrics-server/)).toBeNull();
    });

    it("cordons a node once the person agrees, showing the command it runs", async () => {
        render(<Nodes nodes={[CONTROL]} context="kind-dev" />);
        fireEvent.click(screen.getByRole("button", { name: "Cordon kind-dev-control-plane" }));
        await waitFor(() => expect(api.cordon).toHaveBeenCalledWith("kind-dev", "kind-dev-control-plane", true));
        expect(host.confirmDialog.mock.calls[0][0].commands[0].command).toBe("kubectl --context kind-dev cordon kind-dev-control-plane");
        expect(host.notify).toHaveBeenCalledWith("success", "node/kind-dev-control-plane cordoned");
    });

    it("uncordons a cordoned node, and does nothing when the person says no", async () => {
        host.confirmDialog.mockResolvedValue(false);
        render(<Nodes nodes={[SICK]} context="kind-dev" />);
        fireEvent.click(screen.getByRole("button", { name: "Uncordon kind-dev-worker" }));
        await waitFor(() => expect(host.confirmDialog).toHaveBeenCalled());
        expect(api.cordon).not.toHaveBeenCalled();
    });
});
