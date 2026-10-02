import { beforeEach, describe, expect, it, vi } from "vitest";
import { performanceTelemetry } from "../lib/performance";
import { ProjectController, ProjectControllerRegistry, type ProjectControllerServices } from "./projectController";

type Config = { name: string };
type Worktree = { path: string };
const FIRST_WATCH_TOKEN = "00000000-0000-4000-8000-000000000001";
const SECOND_WATCH_TOKEN = "00000000-0000-4000-8000-000000000002";

function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (error: unknown) => void;
    const promise = new Promise<T>((done, fail) => {
        resolve = done;
        reject = fail;
    });
    return { promise, resolve, reject };
}

function services() {
    let watchSequence = 0;
    const api = {
        loadConfig: vi.fn<(cwd: string) => Promise<Config>>().mockResolvedValue({ name: "repo" }),
        loadWorktrees: vi.fn<(cwd: string) => Promise<readonly Worktree[]>>().mockResolvedValue([{ path: "/repo" }]),
        newWatchLeaseToken: vi.fn(() => (watchSequence++ === 0 ? FIRST_WATCH_TOKEN : SECOND_WATCH_TOKEN)),
        watchStart: vi.fn<ProjectControllerServices<Config, Worktree>["watchStart"]>(),
        watchStop: vi.fn<ProjectControllerServices<Config, Worktree>["watchStop"]>(),
    } satisfies ProjectControllerServices<Config, Worktree>;
    return api;
}

beforeEach(() => performanceTelemetry.reset());

describe("ProjectController", () => {
    it("starts shared services once and stops them after the final release", async () => {
        const api = services();
        const registry = new ProjectControllerRegistry(api);
        const first = registry.acquire("/repo");
        const second = registry.acquire("/repo");
        expect(second.controller).toBe(first.controller);
        await first.ready;

        expect(api.watchStart).toHaveBeenCalledTimes(1);
        expect(first.controller.getSnapshot()).toMatchObject({ status: "ready", retainCount: 2, revision: 1 });
        first.release();
        first.release();
        expect(api.watchStop).not.toHaveBeenCalled();
        second.release();
        expect(api.watchStop).toHaveBeenCalledTimes(1);
    });

    it("coalesces refresh bursts and ignores stale work after release", async () => {
        const config = deferred<Config>();
        const worktrees = deferred<readonly Worktree[]>();
        const api = services();
        api.loadConfig.mockReturnValueOnce(config.promise);
        api.loadWorktrees.mockReturnValueOnce(worktrees.promise);
        const controller = new ProjectController("/repo", api);
        const first = controller.retain();
        const second = controller.refresh();
        expect(second).toBe(first);

        controller.release();
        config.resolve({ name: "stale" });
        worktrees.resolve([{ path: "/stale" }]);
        await first;

        expect(controller.getSnapshot()).toMatchObject({ retainCount: 0, config: null, worktrees: [] });
        expect(api.loadConfig).toHaveBeenCalledTimes(1);
    });

    it("serializes a deferred watcher start before compensating a release", async () => {
        const start = deferred<void>();
        const stopped = deferred<void>();
        const api = services();
        api.watchStart.mockImplementationOnce(() => start.promise);
        api.watchStop.mockImplementationOnce(() => {
            stopped.resolve(undefined);
        });
        const controller = new ProjectController("/repo", api);

        await controller.retain();
        controller.release();
        expect(api.watchStop).not.toHaveBeenCalled();

        start.resolve(undefined);
        await stopped.promise;
        expect(api.watchStart).toHaveBeenCalledTimes(1);
        expect(api.watchStop).toHaveBeenCalledTimes(1);
        expect(controller.getSnapshot().retainCount).toBe(0);
    });

    it("coalesces release and reacquire while a watcher start is pending", async () => {
        const start = deferred<void>();
        const stopped = deferred<void>();
        const api = services();
        api.watchStart.mockImplementationOnce(() => start.promise);
        api.watchStop.mockImplementationOnce(() => {
            stopped.resolve(undefined);
        });
        const controller = new ProjectController("/repo", api);

        await controller.retain();
        controller.release();
        await controller.retain();
        start.resolve(undefined);
        await start.promise;
        await Promise.resolve();

        expect(api.watchStart).toHaveBeenCalledTimes(1);
        expect(api.watchStop).not.toHaveBeenCalled();
        controller.release();
        await stopped.promise;
        expect(api.watchStop).toHaveBeenCalledTimes(1);
    });

    it("compensates a lost start response and keeps that failure visible after a slower refresh", async () => {
        const failedStart = deferred<void>();
        const failurePublished = deferred<void>();
        const restarted = deferred<void>();
        const config = deferred<Config>();
        const worktrees = deferred<readonly Worktree[]>();
        const api = services();
        api.loadConfig.mockReturnValueOnce(config.promise);
        api.loadWorktrees.mockReturnValueOnce(worktrees.promise);
        api.watchStart
            .mockImplementationOnce(() => failedStart.promise)
            .mockImplementationOnce(() => {
                restarted.resolve(undefined);
            });
        const controller = new ProjectController("/repo", api);
        controller.subscribe(() => {
            if (controller.getSnapshot().status === "error") failurePublished.resolve(undefined);
        });

        const ready = controller.retain();
        failedStart.reject(new Error("watch start response lost"));
        await failurePublished.promise;
        expect(controller.getSnapshot()).toMatchObject({ status: "error", error: "watch start response lost", retainCount: 1 });
        config.resolve({ name: "fresh" });
        worktrees.resolve([{ path: "/fresh" }]);
        await ready;
        expect(controller.getSnapshot()).toMatchObject({
            status: "error",
            error: "watch start response lost",
            config: { name: "fresh" },
            worktrees: [{ path: "/fresh" }],
            revision: 1,
        });
        expect(api.watchStart).toHaveBeenCalledTimes(1);
        expect(api.watchStart.mock.calls).toEqual([["/repo", FIRST_WATCH_TOKEN]]);
        expect(api.watchStop.mock.calls).toEqual([[FIRST_WATCH_TOKEN]]);
        expect(performanceTelemetry.snapshot().counters["project.watch.start.failures"]).toBe(1);
        expect(performanceTelemetry.snapshot().counters["project.watch.start.compensations"]).toBe(1);

        controller.release();
        await controller.retain();
        await restarted.promise;
        expect(api.watchStart).toHaveBeenCalledTimes(2);
        expect(api.watchStart.mock.calls[1]).toEqual(["/repo", SECOND_WATCH_TOKEN]);
        expect(controller.getSnapshot()).toMatchObject({ status: "ready", error: null, retainCount: 1 });
    });

    it("retries the same exact lease token after a lost final-stop response", async () => {
        const stopped = deferred<void>();
        const api = services();
        api.watchStop.mockRejectedValueOnce(new Error("temporary stop failure")).mockImplementationOnce(() => stopped.resolve(undefined));
        const controller = new ProjectController("/repo", api);

        await controller.retain();
        controller.dispose();
        await stopped.promise;

        expect(api.watchStop).toHaveBeenCalledTimes(2);
        expect(api.watchStop.mock.calls).toEqual([[FIRST_WATCH_TOKEN], [FIRST_WATCH_TOKEN]]);
        expect(performanceTelemetry.snapshot().counters).toMatchObject({
            "project.watch.stop.failures": 1,
            "project.watch.stop.retries": 1,
        });
    });

    it("finishes an exact-token stop retry before a concurrent reacquire", async () => {
        const lostStopResponse = deferred<void>();
        const replacementStarted = deferred<void>();
        const api = services();
        api.watchStart
            .mockImplementationOnce(() => undefined)
            .mockImplementationOnce(() => {
                replacementStarted.resolve(undefined);
            });
        api.watchStop.mockReturnValueOnce(lostStopResponse.promise).mockResolvedValueOnce(undefined);
        const controller = new ProjectController("/repo", api);

        await controller.retain();
        controller.release();
        const refreshed = controller.retain();
        lostStopResponse.reject(new Error("response lost after native token consumption"));
        await replacementStarted.promise;
        await refreshed;

        expect(api.watchStop.mock.calls).toEqual([[FIRST_WATCH_TOKEN], [FIRST_WATCH_TOKEN]]);
        expect(api.watchStart.mock.calls).toEqual([
            ["/repo", FIRST_WATCH_TOKEN],
            ["/repo", SECOND_WATCH_TOKEN],
        ]);
        expect(controller.getSnapshot()).toMatchObject({ status: "ready", error: null, retainCount: 1 });
    });

    it("reasserts the same uncertain token after all bounded stop responses are lost", async () => {
        const exhausted = deferred<void>();
        const reasserted = deferred<void>();
        const api = services();
        api.watchStart.mockImplementationOnce(() => undefined).mockImplementationOnce(() => reasserted.resolve(undefined));
        api.watchStop.mockImplementation(() => {
            if (api.watchStop.mock.calls.length === 3) exhausted.resolve(undefined);
            return Promise.reject(new Error("persistent stop failure"));
        });
        const controller = new ProjectController("/repo", api);

        await controller.retain();
        controller.release();
        await exhausted.promise;
        await Promise.resolve();

        expect(api.watchStop).toHaveBeenCalledTimes(3);
        expect(api.watchStop.mock.calls).toEqual([[FIRST_WATCH_TOKEN], [FIRST_WATCH_TOKEN], [FIRST_WATCH_TOKEN]]);
        expect(performanceTelemetry.snapshot().counters).toMatchObject({
            "project.watch.stop.failures": 3,
            "project.watch.stop.retries": 2,
            "project.watch.stop.exhausted": 1,
        });

        await controller.retain();
        await reasserted.promise;
        expect(api.newWatchLeaseToken).toHaveBeenCalledTimes(1);
        expect(api.watchStart.mock.calls).toEqual([
            ["/repo", FIRST_WATCH_TOKEN],
            ["/repo", FIRST_WATCH_TOKEN],
        ]);
        expect(controller.getSnapshot()).toMatchObject({ status: "ready", error: null, retainCount: 1 });
    });

    it("compensates a watcher start that resolves after disposal", async () => {
        const start = deferred<void>();
        const stopped = deferred<void>();
        const api = services();
        api.watchStart.mockImplementationOnce(() => start.promise);
        api.watchStop.mockImplementationOnce(() => {
            stopped.resolve(undefined);
        });
        const controller = new ProjectController("/repo", api);

        await controller.retain();
        controller.dispose();
        controller.dispose();
        expect(api.watchStop).not.toHaveBeenCalled();
        start.resolve(undefined);
        await stopped.promise;

        expect(api.watchStart).toHaveBeenCalledTimes(1);
        expect(api.watchStop).toHaveBeenCalledTimes(1);
        expect(controller.getSnapshot()).toMatchObject({ status: "idle", retainCount: 0 });
    });

    it("queues a fresh generation when released and reacquired before stale work settles", async () => {
        const staleConfig = deferred<Config>();
        const staleWorktrees = deferred<readonly Worktree[]>();
        const freshConfig = deferred<Config>();
        const freshWorktrees = deferred<readonly Worktree[]>();
        const freshStarted = deferred<void>();
        const api = services();
        api.loadConfig
            .mockReset()
            .mockReturnValueOnce(staleConfig.promise)
            .mockImplementationOnce(() => {
                freshStarted.resolve(undefined);
                return freshConfig.promise;
            });
        api.loadWorktrees.mockReset().mockReturnValueOnce(staleWorktrees.promise).mockReturnValueOnce(freshWorktrees.promise);
        const controller = new ProjectController("/repo", api);

        const firstReady = controller.retain();
        controller.release();
        const reacquiredReady = controller.retain();

        expect(reacquiredReady).toBe(firstReady);
        expect(api.watchStart).toHaveBeenCalledTimes(1);
        expect(api.watchStop).not.toHaveBeenCalled();
        expect(controller.getSnapshot()).toMatchObject({ status: "loading", retainCount: 1, config: null, worktrees: [] });

        staleConfig.resolve({ name: "stale" });
        staleWorktrees.resolve([{ path: "/stale" }]);
        await freshStarted.promise;

        expect(api.loadConfig).toHaveBeenCalledTimes(2);
        expect(api.loadWorktrees).toHaveBeenCalledTimes(2);
        expect(controller.getSnapshot()).toMatchObject({ status: "loading", retainCount: 1, config: null, worktrees: [], revision: 0 });
        freshConfig.resolve({ name: "fresh" });
        freshWorktrees.resolve([{ path: "/fresh" }]);
        await reacquiredReady;

        expect(controller.getSnapshot()).toMatchObject({
            status: "ready",
            retainCount: 1,
            config: { name: "fresh" },
            worktrees: [{ path: "/fresh" }],
            revision: 1,
        });
        expect(performanceTelemetry.snapshot().counters).toMatchObject({
            "project.refresh.queued": 1,
            "project.refresh.cancelled": 1,
        });
        expect(performanceTelemetry.snapshot().latencies["project.refresh"].count).toBe(2);
    });

    it("disposal cancels queued replacement work and ignores the in-flight result", async () => {
        const config = deferred<Config>();
        const worktrees = deferred<readonly Worktree[]>();
        const api = services();
        api.loadConfig.mockReturnValueOnce(config.promise);
        api.loadWorktrees.mockReturnValueOnce(worktrees.promise);
        const controller = new ProjectController("/repo", api);

        const ready = controller.retain();
        expect(controller.refresh()).toBe(ready);
        controller.dispose();
        controller.dispose();
        config.resolve({ name: "late" });
        worktrees.resolve([{ path: "/late" }]);
        await ready;

        expect(api.loadConfig).toHaveBeenCalledTimes(1);
        expect(api.loadWorktrees).toHaveBeenCalledTimes(1);
        expect(api.watchStop).toHaveBeenCalledTimes(1);
        expect(controller.getSnapshot()).toMatchObject({ status: "idle", retainCount: 0, config: null, worktrees: [], revision: 0 });
        expect(performanceTelemetry.snapshot().counters).toMatchObject({
            "project.refresh.queued": 1,
            "project.refresh.cancelled": 1,
        });
        await expect(controller.refresh()).resolves.toBeUndefined();
        expect(() => controller.retain()).toThrow("disposed");
    });
});
