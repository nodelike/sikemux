import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { performanceTelemetry } from "../lib/performance";
import type { Session, Window } from "../state/types";
import { captureWorkbenchItemRuntimeLease, getOrCreateWorkbenchItemResource, resetWorkbenchItemRuntimeForTests } from "./itemRuntime";
import { createItemId, type WorkbenchItemController, type WorkbenchItemRef, type WorkbenchItemRegistry } from "./registry";
import { SESSION_ITEM_LIFECYCLE_FAILURE_CAPACITY, SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS, SessionController } from "./sessionController";

const session = (activeWindowId = "window-1"): Session => ({
    id: "session-1",
    name: "repo",
    kind: "project",
    cwd: "/repo",
    pinned: false,
    activeWindowId,
});

const window = (paneId: string, kind: "terminal" | "editor" = "terminal"): Window => ({
    id: "window-1",
    name: "term",
    role: "term",
    activePaneId: paneId,
    root: { type: "pane", id: paneId, cwd: "/repo", kind, title: paneId },
});

function deferred<T = void>() {
    let resolve!: (value: T | PromiseLike<T>) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

function lifecycleController(overrides: Partial<WorkbenchItemController> = {}): WorkbenchItemController {
    return {
        activate: () => {},
        deactivate: () => {},
        canClose: () => true,
        dispose: () => {},
        ...overrides,
    };
}

function registry(factory: (ref: WorkbenchItemRef) => WorkbenchItemController) {
    const createSpy = vi.fn(factory);
    const create = ((ref: WorkbenchItemRef) => createSpy(ref)) as WorkbenchItemRegistry["create"];
    return { create, createSpy };
}

beforeEach(() => performanceTelemetry.reset());
afterEach(() => resetWorkbenchItemRuntimeForTests());

describe("SessionController lifecycle", () => {
    it("creates once, converges activation, and disposes removed items", async () => {
        const events: string[] = [];
        const controllers = new Map<string, WorkbenchItemController>();
        const itemRegistry = registry((ref) => {
            const controller = lifecycleController({
                activate: () => {
                    events.push(`activate:${ref.id}`);
                },
                deactivate: () => {
                    events.push(`deactivate:${ref.id}`);
                },
                dispose: () => {
                    events.push(`dispose:${ref.id}`);
                },
            });
            controllers.set(ref.id, controller);
            return controller;
        });
        const controller = new SessionController("session-1", itemRegistry);

        controller.reconcile(session(), [window("pane-1")], "session-1");
        expect(controller.getSnapshot().items[0]).toMatchObject({ state: "activating", desired: "active" });
        await controller.whenIdle();
        controller.reconcile(session(), [window("pane-1")], "session-1");
        expect(events).toEqual(["activate:pane-1"]);

        controller.reconcile(session(), [window("pane-2", "editor")], "session-1");
        await controller.whenIdle();

        expect(itemRegistry.createSpy).toHaveBeenCalledTimes(2);
        expect(events).toEqual(["activate:pane-1", "deactivate:pane-1", "activate:pane-2", "dispose:pane-1"]);
        expect(controller.getSnapshot()).toMatchObject({ itemCount: 1, activeItemId: "pane-2", retiringItems: 0 });
        expect(controllers.has("pane-1")).toBe(true);
    });

    it("deactivates when backgrounded and disposes idempotently", async () => {
        const activate = vi.fn();
        const deactivate = vi.fn();
        const dispose = vi.fn();
        const controller = new SessionController(
            "session-1",
            registry(() => lifecycleController({ activate, deactivate, dispose })),
        );
        controller.reconcile(session(), [window("pane-1")], "session-1");
        await controller.whenIdle();

        controller.reconcile(session(), [window("pane-1")], "another-session");
        expect(controller.getSnapshot().items[0]).toMatchObject({ state: "deactivating", desired: "inactive" });
        await controller.whenIdle();
        controller.dispose();
        controller.dispose();
        await controller.whenIdle();

        expect(activate).toHaveBeenCalledOnce();
        expect(deactivate).toHaveBeenCalledOnce();
        expect(dispose).toHaveBeenCalledOnce();
        expect(controller.getSnapshot()).toMatchObject({ itemCount: 0, activeItemId: null, disposed: true, retiringItems: 0 });
    });
});

describe("SessionController retries and generations", () => {
    it("records an activation rejection, retries once, and becomes active", async () => {
        const failure = new Error("activate failed once");
        const activate = vi.fn<WorkbenchItemController["activate"]>().mockRejectedValueOnce(failure).mockResolvedValueOnce();
        const controller = new SessionController(
            "session-1",
            registry(() => lifecycleController({ activate })),
        );

        controller.reconcile(session(), [window("pane-1")], "session-1");
        const snapshot = await controller.whenIdle();

        expect(activate).toHaveBeenCalledTimes(2);
        expect(snapshot.items[0]).toMatchObject({
            state: "active",
            desired: "active",
            attempts: 2,
            failedOperation: null,
            error: null,
        });
        expect(snapshot).toMatchObject({ failedItems: 0, failureCount: 1 });
        expect(snapshot.failures[0]).toMatchObject({ operation: "activate", attempt: 1, stale: false, message: "activate failed once" });

        const telemetry = performanceTelemetry.snapshot();
        expect(telemetry.counters).toMatchObject({
            "workbench.item.lifecycle.activate.attempts": 2,
            "workbench.item.lifecycle.activate.errors": 1,
            "workbench.item.lifecycle.activate.retries": 1,
            "workbench.item.lifecycle.activate.success": 1,
        });
        expect(telemetry.latencies["workbench.item.lifecycle.activate"].count).toBe(2);
    });

    it("caps automatic retries per generation and only retries again explicitly", async () => {
        const activate = vi.fn<WorkbenchItemController["activate"]>().mockRejectedValue(new Error("always fails"));
        const controller = new SessionController(
            "session-1",
            registry(() => lifecycleController({ activate })),
        );
        controller.reconcile(session(), [window("pane-1")], "session-1");

        let snapshot = await controller.whenIdle();
        expect(activate).toHaveBeenCalledTimes(SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS);
        expect(snapshot.items[0]).toMatchObject({
            state: "failed",
            attempts: SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS,
            failedOperation: "activate",
        });

        controller.reconcile(session(), [window("pane-1")], "session-1");
        await controller.whenIdle();
        expect(activate).toHaveBeenCalledTimes(SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS);

        const previousGeneration = snapshot.items[0].generation;
        expect(controller.retryFailed(snapshot.items[0].id)).toBe(true);
        snapshot = await controller.whenIdle();
        expect(snapshot.items[0].generation).toBe(previousGeneration + 1);
        expect(activate).toHaveBeenCalledTimes(SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS * 2);
        expect(snapshot.items[0].state).toBe("failed");
    });

    it("treats an old activation completion as stale and converges current intent", async () => {
        const activation = deferred();
        const activate = vi.fn<WorkbenchItemController["activate"]>().mockReturnValue(activation.promise);
        const deactivate = vi.fn<WorkbenchItemController["deactivate"]>();
        const controller = new SessionController(
            "session-1",
            registry(() => lifecycleController({ activate, deactivate })),
        );

        controller.reconcile(session(), [window("pane-1")], "session-1");
        const activating = controller.getSnapshot().items[0];
        controller.reconcile(session(), [window("pane-1")], "another-session");
        const changed = controller.getSnapshot().items[0];
        expect(changed).toMatchObject({ state: "activating", desired: "inactive" });
        expect(changed.generation).toBeGreaterThan(activating.generation);
        expect(changed.pendingGeneration).toBe(activating.pendingGeneration);

        activation.resolve();
        const settled = await controller.whenIdle();

        expect(activate).toHaveBeenCalledOnce();
        expect(deactivate).toHaveBeenCalledOnce();
        expect(settled.items[0]).toMatchObject({ state: "inactive", desired: "inactive", pendingOperation: null });
        expect(performanceTelemetry.snapshot().counters["workbench.item.lifecycle.activate.stale"]).toBe(1);
    });

    it("bounds retained failure history without reusing lifecycle generations", async () => {
        const activate = vi.fn<WorkbenchItemController["activate"]>().mockRejectedValue(new Error("bounded failure"));
        const controller = new SessionController(
            "session-1",
            registry(() => lifecycleController({ activate })),
        );
        controller.reconcile(session(), [window("pane-1")], "session-1");
        let snapshot = await controller.whenIdle();

        const generations = SESSION_ITEM_LIFECYCLE_FAILURE_CAPACITY / SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS + 2;
        for (let index = 0; index < generations; index += 1) {
            expect(controller.retryFailed(snapshot.items[0].id)).toBe(true);
            snapshot = await controller.whenIdle();
        }

        expect(snapshot.failureCount).toBeGreaterThan(SESSION_ITEM_LIFECYCLE_FAILURE_CAPACITY);
        expect(snapshot.failures).toHaveLength(SESSION_ITEM_LIFECYCLE_FAILURE_CAPACITY);
        expect(snapshot.failures[0].sequence).toBe(snapshot.failureCount - SESSION_ITEM_LIFECYCLE_FAILURE_CAPACITY + 1);
        expect(Object.isFrozen(snapshot.failures)).toBe(true);
        expect(Object.isFrozen(snapshot.failures[0])).toBe(true);
    });
});

describe("SessionController removal failures", () => {
    it("observes bounded deactivation, controller-dispose, and resource-dispose failures", async () => {
        const deactivateError = new Error("deactivate failed");
        const disposeError = new Error("controller dispose failed");
        const resourceError = new Error("resource dispose failed");
        const deactivate = vi.fn<WorkbenchItemController["deactivate"]>().mockRejectedValue(deactivateError);
        const dispose = vi.fn<WorkbenchItemController["dispose"]>().mockRejectedValue(disposeError);
        const controller = new SessionController(
            "session-1",
            registry(() => lifecycleController({ deactivate, dispose })),
        );
        controller.reconcile(session(), [window("pane-1")], "session-1");
        await controller.whenIdle();
        const runtimeLease = captureWorkbenchItemRuntimeLease(createItemId("pane-1"));
        expect(runtimeLease).not.toBeNull();
        getOrCreateWorkbenchItemResource(runtimeLease!, "test.resource", "test:v1", () => ({
            value: {},
            dispose: () => Promise.reject(resourceError),
        }));

        controller.reconcile(session(), [], "session-1");
        const snapshot = await controller.whenIdle();

        expect(deactivate).toHaveBeenCalledTimes(SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS);
        expect(dispose).toHaveBeenCalledOnce();
        expect(snapshot).toMatchObject({ itemCount: 0, retiringItems: 0, pendingOperations: 0, failedItems: 0 });
        expect(snapshot.failures.filter(({ operation }) => operation === "deactivate")).toHaveLength(2);
        expect(snapshot.failures).toEqual(
            expect.arrayContaining([
                expect.objectContaining({ operation: "dispose", message: "controller dispose failed" }),
                expect.objectContaining({ operation: "resources", message: "Failed to dispose runtime resources for pane-1" }),
            ]),
        );

        const telemetry = performanceTelemetry.snapshot();
        expect(telemetry.counters).toMatchObject({
            "workbench.item.lifecycle.deactivate.attempts": 2,
            "workbench.item.lifecycle.deactivate.errors": 2,
            "workbench.item.lifecycle.deactivate.retries": 1,
            "workbench.item.lifecycle.dispose.attempts": 1,
            "workbench.item.lifecycle.dispose.errors": 1,
            "workbench.item.lifecycle.resources.attempts": 1,
            "workbench.item.lifecycle.resources.errors": 1,
        });
        expect(telemetry.latencies["workbench.item.lifecycle.dispose"].count).toBe(1);
        expect(telemetry.latencies["workbench.item.lifecycle.resources"].count).toBe(1);
    });

    it("waits for stale activation before deactivation and exactly-once session disposal", async () => {
        const activation = deferred();
        const activate = vi.fn<WorkbenchItemController["activate"]>().mockReturnValue(activation.promise);
        const deactivate = vi.fn<WorkbenchItemController["deactivate"]>();
        const dispose = vi.fn<WorkbenchItemController["dispose"]>();
        const controller = new SessionController(
            "session-1",
            registry(() => lifecycleController({ activate, deactivate, dispose })),
        );
        controller.reconcile(session(), [window("pane-1")], "session-1");

        controller.dispose();
        controller.dispose();
        expect(deactivate).not.toHaveBeenCalled();
        expect(dispose).not.toHaveBeenCalled();
        activation.resolve();
        const snapshot = await controller.whenIdle();

        expect(deactivate).toHaveBeenCalledOnce();
        expect(dispose).toHaveBeenCalledOnce();
        expect(snapshot).toMatchObject({ disposed: true, itemCount: 0, retiringItems: 0, pendingOperations: 0 });
    });
});
