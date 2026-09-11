import { beforeEach, describe, expect, it, vi } from "vitest";
import type { GitWorktree } from "../api/git";
import { performanceTelemetry } from "../lib/performance";
import type { ProjectConfigLoadResult } from "../projectConfig";
import { ProjectControllerRuntime, type ProjectControllerRuntimeServices } from "./controllerRuntime";

function watchToken(sequence: number): string {
    return `00000000-0000-4000-8000-${sequence.toString().padStart(12, "0")}`;
}

function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((done, fail) => {
        resolve = done;
        reject = fail;
    });
    return { promise, resolve, reject };
}

function config(root: string, suffix = ""): ProjectConfigLoadResult {
    return { status: "absent", path: `${root}/sikemux${suffix}.json` };
}

function worktree(path: string): GitWorktree {
    return {
        path,
        head: null,
        branch: "main",
        reference: null,
        detached: false,
        locked: false,
        lock_reason: null,
        prunable: false,
        prune_reason: null,
        bare: false,
        current: true,
        is_main: true,
    };
}

function services() {
    let watchSequence = 0;
    type ChangeListener = Parameters<ProjectControllerRuntimeServices["subscribeFsChanged"]>[0];
    const fsListeners = new Set<ChangeListener>();
    const gitListeners = new Set<ChangeListener>();
    const fsHistory: ChangeListener[] = [];
    const gitHistory: ChangeListener[] = [];
    const api = {
        loadConfig: vi.fn<(root: string) => Promise<ProjectConfigLoadResult>>().mockImplementation(async (root) => config(root)),
        loadWorktrees: vi.fn<(root: string) => Promise<readonly GitWorktree[]>>().mockImplementation(async (root) => [worktree(root)]),
        newWatchLeaseToken: vi.fn(() => watchToken((watchSequence += 1))),
        watchStart: vi.fn<ProjectControllerRuntimeServices["watchStart"]>().mockResolvedValue(undefined),
        watchStop: vi.fn<ProjectControllerRuntimeServices["watchStop"]>().mockResolvedValue(undefined),
        subscribeFsChanged: vi.fn<ProjectControllerRuntimeServices["subscribeFsChanged"]>().mockImplementation((listener) => {
            fsListeners.add(listener);
            fsHistory.push(listener);
            let active = true;
            return () => {
                if (!active) return;
                active = false;
                fsListeners.delete(listener);
            };
        }),
        subscribeGitRefresh: vi.fn<ProjectControllerRuntimeServices["subscribeGitRefresh"]>().mockImplementation((listener) => {
            gitListeners.add(listener);
            gitHistory.push(listener);
            let active = true;
            return () => {
                if (!active) return;
                active = false;
                gitListeners.delete(listener);
            };
        }),
    } satisfies ProjectControllerRuntimeServices;
    return {
        api,
        fsListeners,
        gitListeners,
        fsHistory,
        gitHistory,
        emitFs: (repo: string, paths?: readonly string[]) => fsListeners.forEach((listener) => listener(repo, paths)),
        emitGit: (repo: string) => gitListeners.forEach((listener) => listener(repo)),
    };
}

async function flushMicrotasks(): Promise<void> {
    for (let index = 0; index < 8; index += 1) await Promise.resolve();
}

beforeEach(() => performanceTelemetry.reset());

describe("ProjectControllerRuntime", () => {
    it("owns one controller lease per bounded, deduplicated local root", async () => {
        const harness = services();
        const runtime = new ProjectControllerRuntime(harness.api, { maxRoots: 2 });
        const listener = vi.fn();
        runtime.subscribe(listener);

        await runtime.start();
        await runtime.reconcile(["relative", "/alpha", "/alpha", "/beta", "/ignored"], "/beta");

        expect(harness.api.watchStart.mock.calls).toEqual([
            ["/alpha", watchToken(1)],
            ["/beta", watchToken(2)],
        ]);
        expect(harness.api.loadConfig.mock.calls).toEqual([["/alpha"], ["/beta"]]);
        expect(runtime.getActiveSnapshot()).toMatchObject({ cwd: "/beta", status: "ready", retainCount: 1, revision: 1 });
        expect(listener).toHaveBeenCalled();
        expect(performanceTelemetry.snapshot().counters["project.runtime.roots.rejected"]).toBe(3);

        await runtime.reconcile(["/beta", "/alpha", "/beta"], "/beta");
        expect(harness.api.watchStart).toHaveBeenCalledTimes(2);
        expect(harness.api.watchStop).not.toHaveBeenCalled();

        runtime.stop();
        await flushMicrotasks();
        expect(harness.api.watchStop.mock.calls).toEqual([[watchToken(1)], [watchToken(2)]]);
        expect(runtime.getActiveSnapshot()).toBeNull();
    });

    it("subscribes once and routes scoped and global invalidations", async () => {
        const harness = services();
        const runtime = new ProjectControllerRuntime(harness.api);

        await runtime.start();
        await runtime.start();
        expect(harness.api.subscribeFsChanged).toHaveBeenCalledTimes(1);
        expect(harness.api.subscribeGitRefresh).toHaveBeenCalledTimes(1);
        await runtime.reconcile(["/alpha", "/beta"], "/alpha");
        harness.api.loadConfig.mockClear();
        harness.api.loadWorktrees.mockClear();

        harness.emitFs("/alpha", ["src/file.ts"]);
        await flushMicrotasks();
        expect(harness.api.loadConfig).not.toHaveBeenCalled();
        expect(harness.api.loadWorktrees).not.toHaveBeenCalled();

        harness.emitFs("/alpha", ["sikemux.json"]);
        await flushMicrotasks();
        expect(harness.api.loadConfig.mock.calls).toEqual([["/alpha"]]);
        expect(harness.api.loadWorktrees.mock.calls).toEqual([["/alpha"]]);

        harness.emitGit("/canonical/alpha");
        await flushMicrotasks();
        expect(harness.api.loadConfig.mock.calls).toEqual([["/alpha"], ["/alpha"], ["/beta"]]);
        expect(harness.api.loadWorktrees.mock.calls).toEqual([["/alpha"], ["/alpha"], ["/beta"]]);

        harness.emitGit("");
        await flushMicrotasks();
        expect(harness.api.loadConfig.mock.calls).toEqual([["/alpha"], ["/alpha"], ["/beta"], ["/alpha"], ["/beta"]]);
        expect(harness.api.loadWorktrees.mock.calls).toEqual([["/alpha"], ["/alpha"], ["/beta"], ["/alpha"], ["/beta"]]);

        runtime.stop();
        expect(harness.fsListeners.size).toBe(0);
        expect(harness.gitListeners.size).toBe(0);
    });

    it("disposes removed roots and ignores stale completion after reacquiring the same path", async () => {
        const harness = services();
        const staleConfig = deferred<ProjectConfigLoadResult>();
        const staleWorktrees = deferred<readonly GitWorktree[]>();
        harness.api.loadConfig
            .mockReset()
            .mockReturnValueOnce(staleConfig.promise)
            .mockImplementation(async (root) => config(root, ".fresh"));
        harness.api.loadWorktrees
            .mockReset()
            .mockReturnValueOnce(staleWorktrees.promise)
            .mockImplementation(async (root) => [worktree(`${root}/fresh`)]);
        const runtime = new ProjectControllerRuntime(harness.api);
        const listener = vi.fn();
        runtime.subscribe(listener);
        await runtime.start();

        const staleReady = runtime.reconcile(["/repo"], "/repo");
        await flushMicrotasks();
        expect(runtime.getActiveSnapshot()).toMatchObject({ cwd: "/repo", status: "loading", revision: 0 });

        await runtime.reconcile([], null);
        expect(runtime.getActiveSnapshot()).toBeNull();
        const freshReady = runtime.reconcile(["/repo"], "/repo");
        await freshReady;
        expect(runtime.getActiveSnapshot()).toMatchObject({
            cwd: "/repo",
            status: "ready",
            config: config("/repo", ".fresh"),
            worktrees: [worktree("/repo/fresh")],
            revision: 1,
        });
        const notificationsBeforeStaleCompletion = listener.mock.calls.length;

        staleConfig.resolve(config("/repo", ".stale"));
        staleWorktrees.resolve([worktree("/repo/stale")]);
        await staleReady;

        expect(runtime.getActiveSnapshot()).toMatchObject({
            config: config("/repo", ".fresh"),
            worktrees: [worktree("/repo/fresh")],
            revision: 1,
        });
        expect(listener).toHaveBeenCalledTimes(notificationsBeforeStaleCompletion);
        expect(harness.api.watchStart).toHaveBeenCalledTimes(2);
        expect(harness.api.watchStop).toHaveBeenCalledTimes(1);
    });

    it("stops idempotently, ignores stale event callbacks, and can restart with fresh owners", async () => {
        const harness = services();
        const runtime = new ProjectControllerRuntime(harness.api);
        await runtime.start();
        await runtime.reconcile(["/repo"], "/repo");
        const staleFsListener = harness.fsHistory[0]!;
        harness.api.loadConfig.mockClear();

        runtime.stop();
        runtime.stop();
        staleFsListener("/repo");
        await flushMicrotasks();
        expect(harness.api.loadConfig).not.toHaveBeenCalled();
        expect(harness.api.watchStop).toHaveBeenCalledTimes(1);
        expect(runtime.getActiveSnapshot()).toBeNull();

        await runtime.start();
        await runtime.reconcile(["/repo"], "/repo");
        expect(harness.api.subscribeFsChanged).toHaveBeenCalledTimes(2);
        expect(harness.api.subscribeGitRefresh).toHaveBeenCalledTimes(2);
        expect(harness.api.watchStart).toHaveBeenCalledTimes(2);
        expect(runtime.getActiveSnapshot()).toMatchObject({ cwd: "/repo", status: "ready", retainCount: 1 });
        harness.api.loadConfig.mockClear();
        staleFsListener("/repo");
        await flushMicrotasks();
        expect(harness.api.loadConfig).not.toHaveBeenCalled();

        runtime.dispose();
        runtime.dispose();
        await flushMicrotasks();
        expect(harness.api.watchStop).toHaveBeenCalledTimes(2);
    });

    it("contains service and subscription failures without leaking partial ownership", async () => {
        const harness = services();
        harness.api.loadConfig.mockRejectedValue(new Error("config failed"));
        harness.api.subscribeGitRefresh.mockImplementationOnce(() => {
            throw new Error("subscription failed");
        });
        const runtime = new ProjectControllerRuntime(harness.api);

        await expect(runtime.start()).resolves.toBeUndefined();
        expect(runtime.isStarted).toBe(false);
        expect(harness.fsListeners.size).toBe(0);
        await runtime.reconcile(["/repo"], "/repo");
        expect(harness.api.watchStart).not.toHaveBeenCalled();
        expect(performanceTelemetry.snapshot().counters["project.runtime.start.failures"]).toBe(1);

        await runtime.start();
        await runtime.reconcile(["/repo"], "/repo");
        expect(runtime.getActiveSnapshot()).toMatchObject({ status: "error", error: "config failed", retainCount: 1 });
        harness.api.watchStop.mockRejectedValueOnce(new Error("stop failed"));
        runtime.stop();
        await flushMicrotasks();
        expect(runtime.getActiveSnapshot()).toBeNull();
    });
});
