import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RundeckExecution, WatchUpdate } from "./api";

const api = vi.hoisted(() => ({ watchStart: vi.fn(), watchStop: vi.fn() }));
const host = vi.hoisted(() => ({ notify: vi.fn(), notifyDesktop: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({ ...(await importOriginal<object>()), rundeckApi: api }));
vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { watchRun } from "./runWatch";
import { updateRundeckSettings } from "./state";

const job = { project: "shop", jobId: "j1", name: "api", group: "staging/backend" };
let nextId = 100;
let send: (update: WatchUpdate) => void = () => {};

function execution(id: number, status: string): RundeckExecution {
    return {
        id,
        status,
        customStatus: null,
        user: "ankit",
        project: "shop",
        "date-started": { date: "2026-10-08T10:00:00Z", unixtime: null },
        "date-ended": { date: "2026-10-08T10:01:30Z", unixtime: null },
        permalink: null,
        job: { id: "j1", name: "api", group: "staging/backend", project: "shop", options: { branch: "feature/cart" } },
        argstring: null,
    };
}

const update = (id: number, status: string, terminal: boolean): WatchUpdate => ({
    execution: execution(id, status),
    state: null,
    error: null,
    terminal,
});

beforeEach(() => {
    api.watchStart.mockImplementation(async (_id: number, onUpdate: (u: WatchUpdate) => void) => {
        send = onUpdate;
        return 7;
    });
    api.watchStop.mockResolvedValue(undefined);
    host.notifyDesktop.mockResolvedValue(undefined);
    updateRundeckSettings({ notifyWhenDone: true });
});
afterEach(() => {
    vi.clearAllMocks();
});

describe("watchRun", () => {
    it("says nothing while the run goes, then tells the person it succeeded, on the desktop too when away", async () => {
        const id = ++nextId;
        await watchRun(job, id, () => false);
        send(update(id, "running", false));
        expect(host.notify).not.toHaveBeenCalled();

        send(update(id, "succeeded", true));
        expect(host.notify).toHaveBeenCalledWith(
            "success",
            "Deployed feature/cart to staging/backend",
            expect.objectContaining({ action: expect.anything() }),
        );
        expect(host.notifyDesktop).toHaveBeenCalledWith("Deployed feature/cart to staging/backend", "api · took 1m 30s");
        expect(api.watchStop).toHaveBeenCalledWith(7);

        send(update(id, "succeeded", true));
        expect(host.notify).toHaveBeenCalledTimes(1);
    });

    it("shows a failure as an error, and keeps to the toast while Sikemux is in front", async () => {
        const id = ++nextId;
        await watchRun(job, id, () => true);
        send(update(id, "failed", true));
        expect(host.notify).toHaveBeenCalledWith("error", "Deploy of feature/cart to staging/backend failed", expect.anything());
        expect(host.notifyDesktop).not.toHaveBeenCalled();
    });

    it("watches a run once however many times it is asked", async () => {
        const id = ++nextId;
        await watchRun(job, id);
        await watchRun(job, id);
        expect(api.watchStart).toHaveBeenCalledTimes(1);
    });

    it("says how the run ended from its steps when Rundeck stopped returning the run itself", async () => {
        const id = ++nextId;
        await watchRun(job, id, () => true);
        send({ execution: null, state: { executionState: "FAILED", steps: [], stepCount: 2, completed: true }, error: "502", terminal: true });
        expect(host.notify).toHaveBeenCalledWith("error", "api on staging/backend failed", expect.anything());
    });

    it("says it lost track of a run when Rundeck stopped answering altogether", async () => {
        const id = ++nextId;
        await watchRun(job, id, () => false);
        send({ execution: null, state: null, error: "connection refused", terminal: true });
        expect(host.notify).toHaveBeenCalledWith("error", "Lost track of staging/backend/api", expect.anything());
        expect(host.notifyDesktop).toHaveBeenCalledWith(
            "Lost track of staging/backend/api",
            `Rundeck stopped answering before run #${id} ended. Check it in Rundeck.`,
        );
        expect(api.watchStop).toHaveBeenCalledWith(7);
    });

    it("does not watch at all when the person turned the notice off", async () => {
        updateRundeckSettings({ notifyWhenDone: false });
        await watchRun(job, ++nextId, () => false);
        expect(api.watchStart).not.toHaveBeenCalled();
    });

    it("stays quiet when the notice is turned off while a run goes", async () => {
        const id = ++nextId;
        await watchRun(job, id, () => false);
        updateRundeckSettings({ notifyWhenDone: false });
        send(update(id, "failed", true));
        expect(host.notify).not.toHaveBeenCalled();
        expect(host.notifyDesktop).not.toHaveBeenCalled();
        expect(api.watchStop).toHaveBeenCalledWith(7);
    });
});
