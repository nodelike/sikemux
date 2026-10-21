import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NIGHTLY, WEB, workload } from "../testing";

const api = vi.hoisted(() => ({ act: vi.fn() }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), promptDialog: vi.fn(), notify: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), kubeApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { actionsFor, replicasOf, WorkloadActions } from "./WorkloadActions";

const scope = { context: "kind-dev", namespace: "shop" };

afterEach(cleanup);
beforeEach(() => {
    Object.values({ ...api, ...host }).forEach((mock) => mock.mockReset());
    api.act.mockResolvedValue({ message: "deployment.apps/web restarted" });
    host.confirmDialog.mockResolvedValue(true);
});

describe("workload actions", () => {
    it("offers only what each kind can do", () => {
        expect(actionsFor(WEB).map((each) => each.action)).toEqual(["restart", "scale"]);
        expect(actionsFor({ kind: "DaemonSet", suspended: null }).map((each) => each.action)).toEqual(["restart"]);
        expect(actionsFor(NIGHTLY).map((each) => each.action)).toEqual(["runNow", "suspend"]);
        expect(actionsFor({ ...NIGHTLY, suspended: true }).map((each) => each.label)).toEqual(["Run now", "Resume"]);
        expect(actionsFor({ kind: "Job", suspended: null })).toEqual([]);
    });

    it("reads a typed replica count, turning away what it can't use", () => {
        expect(replicasOf(" 3 ")).toBe(3);
        expect(replicasOf("0")).toBe(0);
        expect(replicasOf("two")).toMatch(/whole number/);
        expect(replicasOf("-1")).toMatch(/whole number/);
        expect(replicasOf("80")).toMatch(/at most 50/);
    });

    it("restarts once the person agrees, showing the command it runs", async () => {
        render(<WorkloadActions workload={WEB} scope={scope} />);
        fireEvent.click(screen.getByRole("button", { name: "Restart web" }));
        await waitFor(() => expect(api.act).toHaveBeenCalledWith(scope, "restart", "web", "Deployment", undefined));
        expect(host.confirmDialog.mock.calls[0][0].commands[0].command).toBe(
            "kubectl --context kind-dev --namespace shop rollout restart deployment/web",
        );
        expect(host.notify).toHaveBeenCalledWith("success", "deployment.apps/web restarted");
    });

    it("does nothing when the person says no", async () => {
        host.confirmDialog.mockResolvedValue(false);
        render(<WorkloadActions workload={NIGHTLY} scope={scope} />);
        fireEvent.click(screen.getByRole("button", { name: "Run now nightly-report" }));
        await waitFor(() => expect(host.confirmDialog).toHaveBeenCalled());
        expect(api.act).not.toHaveBeenCalled();
    });

    it("scales to the count typed, and asks again before stopping every pod", async () => {
        host.promptDialog.mockResolvedValueOnce("4").mockResolvedValueOnce("0");
        const twoPods = workload("api", { desired: 2 });
        render(<WorkloadActions workload={twoPods} scope={scope} />);
        fireEvent.click(screen.getByRole("button", { name: "Scale api" }));
        await waitFor(() => expect(api.act).toHaveBeenCalledWith(scope, "scale", "api", "Deployment", 4));
        expect(host.confirmDialog).not.toHaveBeenCalled();
        fireEvent.click(screen.getByRole("button", { name: "Scale api" }));
        await waitFor(() => expect(api.act).toHaveBeenLastCalledWith(scope, "scale", "api", "Deployment", 0));
        expect(host.confirmDialog.mock.calls[0][0]).toMatchObject({ destructive: true, confirmLabel: "Scale to 0" });
    });
});
