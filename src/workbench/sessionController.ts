import { performanceTelemetry, type PerformanceSpan } from "../lib/performance";
import { collectPanes } from "../state/layout";
import type { Session, Window } from "../state/types";
import {
    createWorkbenchItemRef,
    workbenchItemRegistry,
    type ItemId,
    type WorkbenchItemController,
    type WorkbenchItemRef,
    type WorkbenchItemRegistry,
} from "./registry";
import { claimWorkbenchItemRuntime, closeWorkbenchItemRuntime, disposeWorkbenchItemRuntime, type WorkbenchItemRuntimeLease } from "./itemRuntime";

export type ItemLifecycleState = "inactive" | "activating" | "active" | "deactivating" | "failed";
export type ItemLifecycleOperation = "activate" | "deactivate" | "dispose" | "resources";

type DesiredLifecycleState = "inactive" | "active" | "disposed";
type SettledLifecycleState = "inactive" | "active";
type TransitionOperation = "activate" | "deactivate";
type LifecycleOutcome = "success" | "error" | "stale-success" | "stale-error";

export const SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS = 2;
export const SESSION_ITEM_LIFECYCLE_FAILURE_CAPACITY = 32;
export const SESSION_ITEM_LIFECYCLE_METRIC = "workbench.item.lifecycle";

export interface ItemLifecycleSnapshot {
    readonly id: ItemId;
    readonly kind: string;
    readonly state: ItemLifecycleState;
    readonly desired: "inactive" | "active";
    readonly generation: number;
    readonly attempts: number;
    readonly pendingOperation: ItemLifecycleOperation | null;
    readonly pendingGeneration: number | null;
    readonly failedOperation: ItemLifecycleOperation | null;
    readonly error: string | null;
}

export interface SessionControllerFailure {
    readonly sequence: number;
    readonly itemId: ItemId;
    readonly kind: string;
    readonly operation: ItemLifecycleOperation;
    readonly generation: number;
    readonly attempt: number;
    readonly stale: boolean;
    readonly message: string;
}

export interface SessionControllerSnapshot {
    readonly sessionId: string;
    readonly itemCount: number;
    readonly activeItemId: ItemId | null;
    readonly disposed: boolean;
    readonly pendingOperations: number;
    readonly retiringItems: number;
    readonly failedItems: number;
    readonly failureCount: number;
    readonly items: readonly ItemLifecycleSnapshot[];
    readonly failures: readonly SessionControllerFailure[];
}

interface ItemRuntime {
    readonly ref: WorkbenchItemRef;
    readonly controller: WorkbenchItemController;
    readonly runtimeLease: WorkbenchItemRuntimeLease;
    desired: DesiredLifecycleState;
    settled: SettledLifecycleState;
    state: ItemLifecycleState;
    generation: number;
    attempts: number;
    pending: Promise<void> | null;
    pendingOperation: ItemLifecycleOperation | null;
    pendingGeneration: number | null;
    failedOperation: ItemLifecycleOperation | null;
    error: string | null;
    retired: boolean;
    disposeStarted: boolean;
    controllerDisposed: boolean;
    resourcesStarted: boolean;
    resourcesSettled: boolean;
}

const ERROR_MESSAGE_LIMIT = 512;

function lifecycleMetric(operation: ItemLifecycleOperation): string {
    return `${SESSION_ITEM_LIFECYCLE_METRIC}.${operation}`;
}

function sanitizedErrorMessage(error: unknown): string {
    let message = "workbench item lifecycle failed";
    try {
        if (error instanceof Error && typeof error.message === "string") message = error.message;
        else if (typeof error === "string") message = error;
    } catch {
        // Hostile getters and proxy errors never escape lifecycle observation.
    }
    let sanitized = "";
    for (const character of message.slice(0, ERROR_MESSAGE_LIMIT)) {
        const code = character.charCodeAt(0);
        sanitized += code <= 31 || (code >= 127 && code <= 159) ? " " : character;
    }
    sanitized = sanitized.trim();
    return (sanitized || "workbench item lifecycle failed").slice(0, ERROR_MESSAGE_LIMIT);
}

function transitionTarget(operation: TransitionOperation): SettledLifecycleState {
    return operation === "activate" ? "active" : "inactive";
}

function transitionState(operation: TransitionOperation): ItemLifecycleState {
    return operation === "activate" ? "activating" : "deactivating";
}

function createItemRuntime(ref: WorkbenchItemRef, controller: WorkbenchItemController, runtimeLease: WorkbenchItemRuntimeLease): ItemRuntime {
    return {
        ref,
        controller,
        runtimeLease,
        desired: "inactive",
        settled: "inactive",
        state: "inactive",
        generation: 0,
        attempts: 0,
        pending: null,
        pendingOperation: null,
        pendingGeneration: null,
        failedOperation: null,
        error: null,
        retired: false,
        disposeStarted: false,
        controllerDisposed: false,
        resourcesStarted: false,
        resourcesSettled: false,
    };
}

function createOwnedItemRuntime(ref: WorkbenchItemRef, registry: Pick<WorkbenchItemRegistry, "create">): ItemRuntime {
    const runtimeLease = claimWorkbenchItemRuntime(ref.id);
    try {
        return createItemRuntime(ref, registry.create(ref), runtimeLease);
    } catch (error) {
        void disposeWorkbenchItemRuntime(runtimeLease);
        throw error;
    }
}

/** Owns runtime item controllers for one durable session topology. */
export class SessionController {
    private readonly items = new Map<ItemId, ItemRuntime>();
    private readonly retiring = new Set<ItemRuntime>();
    private readonly pendingOperations = new Set<Promise<void>>();
    private readonly failures: SessionControllerFailure[] = [];
    private activeItemId: ItemId | null = null;
    private failureSequence = 0;
    private disposed = false;

    constructor(
        readonly sessionId: string,
        private readonly registry: Pick<WorkbenchItemRegistry, "create"> = workbenchItemRegistry,
    ) {}

    /** Lets go of items that left this session, so another session can claim them in the same pass. */
    release(windows: readonly Window[]): void {
        if (this.disposed) return;
        const next = new Map<ItemId, WorkbenchItemRef>();
        for (const window of windows) {
            for (const pane of collectPanes(window.root)) {
                const ref = createWorkbenchItemRef(pane.id, pane.kind);
                next.set(ref.id, ref);
            }
        }
        for (const [id, current] of this.items) {
            if (next.get(id)?.kind === current.ref.kind) continue;
            this.items.delete(id);
            this.retire(current);
        }
    }

    reconcile(session: Session, windows: readonly Window[], activeSessionId: string): void {
        if (this.disposed) throw new Error("cannot reconcile a disposed session controller");
        if (session.id !== this.sessionId) throw new Error("session controller received a different session");

        this.release(windows);
        for (const window of windows) {
            for (const pane of collectPanes(window.root)) {
                const ref = createWorkbenchItemRef(pane.id, pane.kind);
                if (!this.items.has(ref.id)) this.items.set(ref.id, createOwnedItemRuntime(ref, this.registry));
            }
        }

        const activeWindow = windows.find((window) => window.id === session.activeWindowId);
        const activePane = activeWindow ? collectPanes(activeWindow.root).find((pane) => pane.id === activeWindow.activePaneId) : undefined;
        const nextActiveId = activeSessionId === session.id && activePane ? createWorkbenchItemRef(activePane.id, activePane.kind).id : null;
        this.activeItemId = nextActiveId && this.items.has(nextActiveId) ? nextActiveId : null;

        for (const [id, current] of this.items) {
            this.setDesired(current, id === this.activeItemId ? "active" : "inactive");
        }
    }

    /** Explicitly starts one new bounded retry generation for an exhausted item. */
    retryFailed(itemId: ItemId): boolean {
        if (this.disposed) return false;
        const runtime = this.items.get(itemId);
        if (!runtime || runtime.state !== "failed" || runtime.pending) return false;
        runtime.generation += 1;
        runtime.attempts = 0;
        runtime.failedOperation = null;
        runtime.error = null;
        runtime.state = runtime.settled;
        this.drive(runtime);
        return true;
    }

    async canClose(): Promise<boolean> {
        for (const item of this.items.values()) {
            if (!(await item.controller.canClose())) return false;
        }
        return true;
    }

    async whenIdle(): Promise<SessionControllerSnapshot> {
        while (this.pendingOperations.size > 0) {
            await Promise.all(Array.from(this.pendingOperations));
        }
        return this.getSnapshot();
    }

    dispose(): void {
        if (this.disposed) return;
        this.disposed = true;
        this.activeItemId = null;
        for (const [id, runtime] of this.items) {
            this.items.delete(id);
            this.retire(runtime);
        }
    }

    getSnapshot(): SessionControllerSnapshot {
        const itemSnapshots = Array.from(this.items.values(), (runtime): ItemLifecycleSnapshot =>
            Object.freeze({
                id: runtime.ref.id,
                kind: runtime.ref.kind,
                state: runtime.state,
                desired: runtime.desired === "active" ? "active" : "inactive",
                generation: runtime.generation,
                attempts: runtime.attempts,
                pendingOperation: runtime.pendingOperation,
                pendingGeneration: runtime.pendingGeneration,
                failedOperation: runtime.failedOperation,
                error: runtime.error,
            }),
        );
        return Object.freeze({
            sessionId: this.sessionId,
            itemCount: this.items.size,
            activeItemId: this.activeItemId,
            disposed: this.disposed,
            pendingOperations: this.pendingOperations.size,
            retiringItems: this.retiring.size,
            failedItems: itemSnapshots.filter((item) => item.state === "failed").length,
            failureCount: this.failureSequence,
            items: Object.freeze(itemSnapshots),
            failures: Object.freeze(this.failures.slice()),
        });
    }

    private setDesired(runtime: ItemRuntime, desired: DesiredLifecycleState): void {
        if (runtime.desired !== desired) {
            runtime.desired = desired;
            runtime.generation += 1;
            runtime.attempts = 0;
            runtime.failedOperation = null;
            runtime.error = null;
            if (!runtime.pending) runtime.state = runtime.settled;
        }
        this.drive(runtime);
    }

    private drive(runtime: ItemRuntime): void {
        if (runtime.pending || runtime.controllerDisposed) return;
        if (runtime.desired === "disposed") {
            if (runtime.settled === "active" && !(runtime.state === "failed" && runtime.attempts >= SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS)) {
                this.startTransition(runtime, "deactivate");
                return;
            }
            this.startControllerDispose(runtime);
            return;
        }

        const target: SettledLifecycleState = runtime.desired;
        if (runtime.settled === target) {
            runtime.state = target;
            runtime.failedOperation = null;
            runtime.error = null;
            return;
        }
        if (runtime.state === "failed" && runtime.attempts >= SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS) return;
        this.startTransition(runtime, target === "active" ? "activate" : "deactivate");
    }

    private startTransition(runtime: ItemRuntime, operation: TransitionOperation): void {
        const operationGeneration = runtime.generation;
        const origin = runtime.settled;
        const attempt = runtime.attempts + 1;
        runtime.attempts = attempt;
        runtime.state = transitionState(operation);
        runtime.failedOperation = null;
        runtime.error = null;
        runtime.pendingOperation = operation;
        runtime.pendingGeneration = operationGeneration;
        const span = this.startObservedOperation(runtime, operation, operationGeneration, attempt);

        let result: void | Promise<void>;
        try {
            result = operation === "activate" ? runtime.controller.activate() : runtime.controller.deactivate();
        } catch (error) {
            result = Promise.reject(error);
        }
        const pending = Promise.resolve(result).then(
            () => this.transitionSucceeded(runtime, operation, operationGeneration, span),
            (error: unknown) => this.transitionFailed(runtime, operation, origin, operationGeneration, attempt, span, error),
        );
        runtime.pending = pending;
        this.track(pending);
    }

    private transitionSucceeded(runtime: ItemRuntime, operation: TransitionOperation, operationGeneration: number, span: PerformanceSpan): void {
        runtime.pending = null;
        runtime.pendingOperation = null;
        runtime.pendingGeneration = null;
        runtime.settled = transitionTarget(operation);
        runtime.state = runtime.settled;
        const stale = operationGeneration !== runtime.generation;
        this.finishObservedOperation(operation, span, stale ? "stale-success" : "success");
        if (!stale) {
            runtime.failedOperation = null;
            runtime.error = null;
        } else {
            performanceTelemetry.incrementCounter(`${lifecycleMetric(operation)}.stale`);
        }
        this.drive(runtime);
    }

    private transitionFailed(
        runtime: ItemRuntime,
        operation: TransitionOperation,
        origin: SettledLifecycleState,
        operationGeneration: number,
        attempt: number,
        span: PerformanceSpan,
        error: unknown,
    ): void {
        runtime.pending = null;
        runtime.pendingOperation = null;
        runtime.pendingGeneration = null;
        runtime.settled = origin;
        const stale = operationGeneration !== runtime.generation;
        this.finishObservedOperation(operation, span, stale ? "stale-error" : "error");
        this.recordFailure(runtime, operation, operationGeneration, attempt, stale, error);
        if (stale) {
            runtime.state = origin;
            performanceTelemetry.incrementCounter(`${lifecycleMetric(operation)}.stale`);
            this.drive(runtime);
            return;
        }

        runtime.state = "failed";
        runtime.failedOperation = operation;
        runtime.error = sanitizedErrorMessage(error);
        if (runtime.attempts < SESSION_ITEM_LIFECYCLE_MAX_ATTEMPTS) {
            performanceTelemetry.incrementCounter(`${lifecycleMetric(operation)}.retries`);
            this.drive(runtime);
        } else if (runtime.desired === "disposed") {
            // Teardown cannot remain stuck forever on a failed deactivation.
            this.drive(runtime);
        }
    }

    private retire(runtime: ItemRuntime): void {
        if (runtime.retired) return;
        runtime.retired = true;
        this.retiring.add(runtime);
        closeWorkbenchItemRuntime(runtime.runtimeLease);
        this.setDesired(runtime, "disposed");
    }

    private startControllerDispose(runtime: ItemRuntime): void {
        if (runtime.disposeStarted) return;
        runtime.disposeStarted = true;
        runtime.pendingOperation = "dispose";
        runtime.pendingGeneration = runtime.generation;
        const span = this.startObservedOperation(runtime, "dispose", runtime.generation, 1);

        let result: void | Promise<void>;
        try {
            result = runtime.controller.dispose();
        } catch (error) {
            result = Promise.reject(error);
        }
        const pending = Promise.resolve(result).then(
            () => {
                runtime.pending = null;
                runtime.pendingOperation = null;
                runtime.pendingGeneration = null;
                runtime.controllerDisposed = true;
                this.finishObservedOperation("dispose", span, "success");
                this.startResourceDisposal(runtime);
                this.finalizeRetired(runtime);
            },
            (error: unknown) => {
                runtime.pending = null;
                runtime.pendingOperation = null;
                runtime.pendingGeneration = null;
                runtime.controllerDisposed = true;
                runtime.state = "failed";
                runtime.failedOperation = "dispose";
                runtime.error = sanitizedErrorMessage(error);
                this.finishObservedOperation("dispose", span, "error");
                this.recordFailure(runtime, "dispose", runtime.generation, 1, false, error);
                this.startResourceDisposal(runtime);
                this.finalizeRetired(runtime);
            },
        );
        runtime.pending = pending;
        this.track(pending);
    }

    private startResourceDisposal(runtime: ItemRuntime): void {
        if (runtime.resourcesStarted) return;
        runtime.resourcesStarted = true;
        const generation = runtime.generation + 1;
        const span = this.startObservedOperation(runtime, "resources", generation, 1);
        const pending = disposeWorkbenchItemRuntime(runtime.runtimeLease).then(
            () => {
                runtime.resourcesSettled = true;
                this.finishObservedOperation("resources", span, "success");
                this.finalizeRetired(runtime);
            },
            (error: unknown) => {
                runtime.resourcesSettled = true;
                this.finishObservedOperation("resources", span, "error");
                this.recordFailure(runtime, "resources", generation, 1, false, error);
                this.finalizeRetired(runtime);
            },
        );
        this.track(pending);
    }

    private startObservedOperation(runtime: ItemRuntime, operation: ItemLifecycleOperation, generation: number, attempt: number): PerformanceSpan {
        const metric = lifecycleMetric(operation);
        performanceTelemetry.incrementCounter(`${metric}.attempts`);
        return performanceTelemetry.startTrace(SESSION_ITEM_LIFECYCLE_METRIC, {
            operation,
            kind: runtime.ref.kind,
            generation,
            attempt,
        });
    }

    private finishObservedOperation(operation: ItemLifecycleOperation, span: PerformanceSpan, outcome: LifecycleOutcome): void {
        const metric = lifecycleMetric(operation);
        const recorded = performanceTelemetry.endSpan(span, { outcome });
        if (recorded) performanceTelemetry.recordLatency(metric, recorded.durationMs);
        if (outcome === "success" || outcome === "stale-success") performanceTelemetry.incrementCounter(`${metric}.success`);
        else if (outcome === "error" || outcome === "stale-error") performanceTelemetry.incrementCounter(`${metric}.errors`);
    }

    private recordFailure(
        runtime: ItemRuntime,
        operation: ItemLifecycleOperation,
        generation: number,
        attempt: number,
        stale: boolean,
        error: unknown,
    ): void {
        this.failureSequence += 1;
        const failure = Object.freeze({
            sequence: this.failureSequence,
            itemId: runtime.ref.id,
            kind: runtime.ref.kind,
            operation,
            generation,
            attempt,
            stale,
            message: sanitizedErrorMessage(error),
        });
        if (this.failures.length >= SESSION_ITEM_LIFECYCLE_FAILURE_CAPACITY) this.failures.shift();
        this.failures.push(failure);
    }

    private track(promise: Promise<void>): void {
        this.pendingOperations.add(promise);
        void promise.then(
            () => this.pendingOperations.delete(promise),
            () => this.pendingOperations.delete(promise),
        );
    }

    private finalizeRetired(runtime: ItemRuntime): void {
        if (runtime.controllerDisposed && runtime.resourcesSettled) this.retiring.delete(runtime);
    }
}
