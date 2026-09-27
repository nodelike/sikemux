import { Channel, type InvokeArgs } from "@tauri-apps/api/core";
import { invokeCommand } from "../api/invoke";
import { createItemId, type ItemId } from "../workbench/registry";
import type {
    TaskExecutionBackend,
    TaskExecutionRequest,
    TaskExecutionStart,
    TaskProcessExit,
    TaskTerminalOpenRequest,
    TaskTerminalSurface,
} from "./runtime";

export const NATIVE_TASK_RUNTIME_LIMITS = Object.freeze({
    maxBindings: 512,
    maxListenersPerPane: 8,
    maxTotalListeners: 1_024,
});

export interface NativeTaskSpawnResult {
    readonly ptyId: number;
}

export interface TaskExitChannel {
    onmessage: (exit: TaskProcessExit) => void;
}

export type TaskCommandInvoker = <Result>(command: string, args?: InvokeArgs) => Promise<Result>;
export type TaskExitChannelFactory = () => TaskExitChannel;

export interface NativeTaskExecutionBackendOptions {
    readonly invoke?: TaskCommandInvoker;
    readonly createExitChannel?: TaskExitChannelFactory;
}

export interface TaskPtyBinding {
    readonly paneId: ItemId;
    readonly ptyId: number;
    readonly executionId: string;
    readonly terminalKey: string;
    readonly revision: number;
}

export interface TaskTerminalPresentationRequest {
    readonly executionId: string;
    readonly terminalKey: string;
    readonly taskId: string;
    readonly label: string;
    readonly project: string;
    readonly source: TaskTerminalOpenRequest["source"];
    readonly cwd: string;
    readonly agentId?: string;
    readonly signal: AbortSignal;
}

export type TaskTerminalPresenter = (request: TaskTerminalPresentationRequest) => string | PromiseLike<string>;

export interface TaskPtyBindingRegistryOptions {
    readonly maxBindings?: number;
    readonly maxListenersPerPane?: number;
    readonly maxTotalListeners?: number;
}

type BindingListener = () => void;

const NOOP = () => {};

function containRejection<Value>(promise: Promise<Value>): Promise<Value> {
    void promise.catch(() => {});
    return promise;
}

function requirePositiveInteger(name: string, value: number | undefined, fallback: number, hardLimit: number): number {
    const resolved = value ?? fallback;
    if (!Number.isSafeInteger(resolved) || resolved <= 0 || resolved > hardLimit) {
        throw new RangeError(`${name} must be a positive integer no greater than ${hardLimit}`);
    }
    return resolved;
}

function requirePtyId(value: unknown): number {
    if (!Number.isSafeInteger(value) || (value as number) < 0 || (value as number) > 0xffff_ffff) {
        throw new TypeError("native task spawn returned an invalid PTY ID");
    }
    return value as number;
}

function abortReason(signal: AbortSignal): unknown {
    return signal.reason ?? new DOMException("Task terminal presentation was aborted", "AbortError");
}

function defaultExitChannel(): TaskExitChannel {
    return new Channel<TaskProcessExit>();
}

/**
 * Native adapter for a task-owned PTY. The Tauri Channel remains a sibling of
 * the structured request because nested channels are not valid CommandArgs.
 */
export class NativeTaskExecutionBackend implements TaskExecutionBackend {
    private readonly invoke: TaskCommandInvoker;
    private readonly createExitChannel: TaskExitChannelFactory;

    constructor(options: NativeTaskExecutionBackendOptions = {}) {
        this.invoke = options.invoke ?? invokeCommand;
        this.createExitChannel = options.createExitChannel ?? defaultExitChannel;
    }

    async start(request: TaskExecutionRequest): Promise<TaskExecutionStart> {
        const exitChannel = this.createExitChannel();
        if (!exitChannel || typeof exitChannel !== "object") throw new TypeError("task exit channel factory returned an invalid channel");

        let settled = false;
        let resolveCompletion!: (exit: TaskProcessExit) => void;
        let rejectCompletion!: (error: unknown) => void;
        const completion = containRejection(
            new Promise<TaskProcessExit>((resolve, reject) => {
                resolveCompletion = resolve;
                rejectCompletion = reject;
            }),
        );
        exitChannel.onmessage = (exit) => {
            if (settled) return;
            settled = true;
            exitChannel.onmessage = NOOP;
            resolveCompletion(exit);
        };

        let result: NativeTaskSpawnResult;
        try {
            result = await this.invoke<NativeTaskSpawnResult>("task_spawn", { request, onExit: exitChannel });
        } catch (error) {
            settled = true;
            exitChannel.onmessage = NOOP;
            rejectCompletion(error);
            throw error;
        }

        return Object.freeze({ ptyId: requirePtyId(result?.ptyId), completion });
    }

    stop(ptyId: number): Promise<void> {
        return this.invoke<void>("pty_kill", { id: requirePtyId(ptyId) });
    }
}

/** Bounded runtime-only map used by terminal renderers to borrow task PTYs. */
export class TaskPtyBindingRegistry {
    private readonly bindings = new Map<ItemId, TaskPtyBinding>();
    private readonly listeners = new Map<ItemId, Set<BindingListener>>();
    private readonly maxBindings: number;
    private readonly maxListenersPerPane: number;
    private readonly maxTotalListeners: number;
    private totalListeners = 0;
    private revision = 0;

    constructor(options: TaskPtyBindingRegistryOptions = {}) {
        this.maxBindings = requirePositiveInteger(
            "maxBindings",
            options.maxBindings,
            NATIVE_TASK_RUNTIME_LIMITS.maxBindings,
            NATIVE_TASK_RUNTIME_LIMITS.maxBindings,
        );
        this.maxListenersPerPane = requirePositiveInteger(
            "maxListenersPerPane",
            options.maxListenersPerPane,
            NATIVE_TASK_RUNTIME_LIMITS.maxListenersPerPane,
            NATIVE_TASK_RUNTIME_LIMITS.maxListenersPerPane,
        );
        this.maxTotalListeners = requirePositiveInteger(
            "maxTotalListeners",
            options.maxTotalListeners,
            NATIVE_TASK_RUNTIME_LIMITS.maxTotalListeners,
            NATIVE_TASK_RUNTIME_LIMITS.maxTotalListeners,
        );
    }

    bind(paneIdInput: string, request: Pick<TaskTerminalOpenRequest, "ptyId" | "executionId" | "terminalKey">): TaskPtyBinding {
        const paneId = createItemId(paneIdInput);
        const existing = this.bindings.get(paneId);
        if (!existing && this.bindings.size >= this.maxBindings) throw new RangeError("task PTY binding capacity reached");
        if (this.revision >= Number.MAX_SAFE_INTEGER) throw new RangeError("task PTY binding revision space exhausted");
        const binding = Object.freeze({
            paneId,
            ptyId: requirePtyId(request.ptyId),
            executionId: request.executionId,
            terminalKey: request.terminalKey,
            revision: ++this.revision,
        });
        this.bindings.set(paneId, binding);
        this.emit(paneId);
        return binding;
    }

    getSnapshot(paneIdInput: string): TaskPtyBinding | null {
        return this.bindings.get(createItemId(paneIdInput)) ?? null;
    }

    release(paneIdInput: string, executionId?: string): boolean {
        const paneId = createItemId(paneIdInput);
        const existing = this.bindings.get(paneId);
        if (!existing || (executionId !== undefined && existing.executionId !== executionId)) return false;
        this.bindings.delete(paneId);
        this.emit(paneId);
        return true;
    }

    subscribe(paneIdInput: string, listener: BindingListener): () => void {
        const paneId = createItemId(paneIdInput);
        if (typeof listener !== "function") throw new TypeError("task PTY binding listener must be a function");
        if (this.totalListeners >= this.maxTotalListeners) throw new RangeError("task PTY binding listener capacity reached");
        const listeners = this.listeners.get(paneId) ?? new Set<BindingListener>();
        if (listeners.size >= this.maxListenersPerPane) throw new RangeError("task PTY binding pane listener capacity reached");
        listeners.add(listener);
        this.listeners.set(paneId, listeners);
        this.totalListeners += 1;
        let subscribed = true;
        return () => {
            if (!subscribed) return;
            subscribed = false;
            if (listeners.delete(listener)) this.totalListeners -= 1;
            if (listeners.size === 0) this.listeners.delete(paneId);
        };
    }

    reset(): void {
        const paneIds = [...this.bindings.keys()];
        this.bindings.clear();
        for (const paneId of paneIds) this.emit(paneId);
    }

    private emit(paneId: ItemId): void {
        for (const listener of [...(this.listeners.get(paneId) ?? [])]) {
            try {
                listener();
            } catch {
                // One renderer cannot prevent other bindings from observing a replacement.
            }
        }
    }
}

/** Opens/reuses a secret-free pane, then binds it to the exact task-owned PTY. */
export class WorkbenchTaskTerminalSurface implements TaskTerminalSurface {
    private readonly bindings: TaskPtyBindingRegistry;
    private readonly present: TaskTerminalPresenter;

    constructor(bindings: TaskPtyBindingRegistry, present: TaskTerminalPresenter) {
        if (!(bindings instanceof TaskPtyBindingRegistry)) throw new TypeError("task terminal bindings must be a TaskPtyBindingRegistry");
        if (typeof present !== "function") throw new TypeError("task terminal presenter must be a function");
        this.bindings = bindings;
        this.present = present;
    }

    async open(request: TaskTerminalOpenRequest): Promise<void> {
        if (request.signal.aborted) throw abortReason(request.signal);
        const presentation = Object.freeze({
            executionId: request.executionId,
            terminalKey: request.terminalKey,
            taskId: request.taskId,
            label: request.label,
            project: request.project,
            source: request.source,
            cwd: request.cwd,
            agentId: request.agentId,
            signal: request.signal,
        });
        const paneId = await this.present(presentation);
        if (request.signal.aborted) throw abortReason(request.signal);
        this.bindings.bind(paneId, request);
    }
}

export const taskPtyBindings = new TaskPtyBindingRegistry();
