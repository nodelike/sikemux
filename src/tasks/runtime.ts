import {
    TASK_REGISTRY_LIMITS,
    TaskController,
    TaskUnavailableError,
    type ResolvedTaskDefinition,
    type TaskControllerOptions,
    type TaskControllerSnapshot,
    type TaskRegistry,
    type TaskRunner,
    type TaskRunnerHandle,
    type TaskSource,
} from "./taskRegistry";

export const TASK_RUNTIME_LIMITS = Object.freeze({
    defaultProjectControllers: 32,
    maxProjectControllers: 128,
    defaultColumns: 80,
    defaultRows: 24,
    maxColumns: 1_000,
    maxRows: 1_000,
    maxPtyId: Number.MAX_SAFE_INTEGER,
    maxSignalLength: 128,
});

/**
 * Complete, trusted launch data for one headless task PTY. The backend must
 * apply `command`, `cwd`, and every `env` entry without shell interpolation or
 * renderer involvement. A rejected start must not transfer process ownership;
 * if native creation has already happened, the backend must clean it up before
 * rejecting.
 */
export interface TaskExecutionRequest {
    readonly executionId: string;
    /** Stable across reruns so the UI can reuse the same command terminal. */
    readonly terminalKey: string;
    readonly taskId: string;
    readonly label: string;
    readonly project: string;
    readonly source: TaskSource;
    readonly command: string;
    readonly cwd: string;
    readonly env: Readonly<Record<string, string>>;
    readonly cols: number;
    readonly rows: number;
    /** The agent whose desk shows the task. */
    readonly agentId?: string;
}

export interface TaskProcessExit {
    readonly code: number;
    readonly signal?: string | null;
}

export interface TaskExecutionStart {
    /** Exact native PTY owned by this run. */
    readonly ptyId: number;
    /** Settles only when the task process, rather than an interactive shell, exits. */
    readonly completion: PromiseLike<TaskProcessExit>;
}

export interface TaskExecutionBackend {
    start(request: TaskExecutionRequest): TaskExecutionStart | PromiseLike<TaskExecutionStart>;
    /** Stop only the PTY returned by `start`; implementations must be idempotent. */
    stop(ptyId: number): void | PromiseLike<void>;
}

/** Intentionally omits command and environment so secrets never enter UI state. */
export interface TaskTerminalOpenRequest {
    readonly executionId: string;
    readonly terminalKey: string;
    readonly ptyId: number;
    readonly taskId: string;
    readonly label: string;
    readonly project: string;
    readonly source: TaskSource;
    readonly cwd: string;
    /** The agent that started the task, whose desk shows its terminal. */
    readonly agentId?: string;
    /** Opened without taking focus, for a task taken back when the app starts. */
    readonly background?: boolean;
    /** Cancels pending presentation on stop/failure; it never requests closing an opened terminal. */
    readonly signal: AbortSignal;
}

export interface TaskTerminalSurface {
    /** Open or reuse a command terminal attached to the already-running PTY. */
    open(request: TaskTerminalOpenRequest): void | PromiseLike<void>;
}

export interface HeadlessPtyTaskRunnerOptions {
    readonly backend: TaskExecutionBackend;
    readonly surface: TaskTerminalSurface;
    readonly cols?: number;
    readonly rows?: number;
}

export interface TaskRuntimeOptions extends HeadlessPtyTaskRunnerOptions {
    readonly registry: TaskRegistry;
    readonly maxProjectControllers?: number;
    readonly controllerOptions?: TaskControllerOptions;
}

export class TaskBackendStartError extends Error {
    constructor() {
        super("Task process could not be started");
        this.name = "TaskBackendStartError";
    }
}

export class TaskBackendProtocolError extends Error {
    constructor(readonly cleanupFailed = false) {
        super(
            cleanupFailed
                ? "Task process backend returned an invalid result and process cleanup failed"
                : "Task process backend returned an invalid result",
        );
        this.name = "TaskBackendProtocolError";
    }
}

export class TaskBackendCompletionError extends Error {
    constructor() {
        super("Task process status could not be observed");
        this.name = "TaskBackendCompletionError";
    }
}

export class TaskBackendStopError extends Error {
    constructor() {
        super("Task process could not be stopped");
        this.name = "TaskBackendStopError";
    }
}

export class TaskTerminalSurfaceError extends Error {
    constructor(readonly cleanupFailed = false) {
        super(cleanupFailed ? "Task terminal could not be opened and its process cleanup failed" : "Task terminal could not be opened");
        this.name = "TaskTerminalSurfaceError";
    }
}

export class TaskProcessExitError extends Error {
    constructor(
        readonly exitCode: number,
        readonly signal: string | null,
    ) {
        super(signal === null ? `Task process exited with code ${exitCode}` : `Task process exited with code ${exitCode} (${signal})`);
        this.name = "TaskProcessExitError";
    }
}

export class TaskRuntimeDisposedError extends Error {
    constructor() {
        super("Task runtime has been disposed");
        this.name = "TaskRuntimeDisposedError";
    }
}

export class TaskRuntimeTaskNotFoundError extends Error {
    constructor() {
        super("Task is not available for this project");
        this.name = "TaskRuntimeTaskNotFoundError";
    }
}

export class TaskRuntimeCapacityError extends Error {
    constructor() {
        super("Task runtime project capacity is occupied by active tasks");
        this.name = "TaskRuntimeCapacityError";
    }
}

export class TaskRuntimeDisposeError extends Error {
    constructor() {
        super("One or more task processes could not be stopped during disposal");
        this.name = "TaskRuntimeDisposeError";
    }
}

type RunState = "active" | "process-settled" | "external-stop" | "surface-failure" | "backend-failure";
interface PendingProcessResult {
    readonly error: TaskProcessExitError | null;
}

const RESOLVED_VOID = Promise.resolve();

function callAsPromise<Value>(operation: () => Value | PromiseLike<Value>): Promise<Value> {
    try {
        return Promise.resolve(operation());
    } catch (error) {
        return Promise.reject(error);
    }
}

/** Preserve identity while making fire-and-forget use rejection-safe. */
function containRejection<Value>(promise: Promise<Value>): Promise<Value> {
    void promise.catch(() => {});
    return promise;
}

function rejected<Value>(error: unknown): Promise<Value> {
    return containRejection(Promise.reject(error));
}

function requirePositiveInteger(name: string, value: number | undefined, fallback: number, hardLimit: number): number {
    const resolved = value ?? fallback;
    if (!Number.isSafeInteger(resolved) || resolved <= 0 || resolved > hardLimit) {
        throw new RangeError(`${name} must be a positive integer no greater than ${hardLimit}`);
    }
    return resolved;
}

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function requireProject(value: unknown): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > TASK_REGISTRY_LIMITS.maxProjectLength ||
        value.trim().length === 0 ||
        containsControlCharacter(value)
    ) {
        throw new TypeError("task project must be bounded non-blank text without control characters");
    }
    return value;
}

function requirePtyId(value: unknown): number {
    if (!Number.isSafeInteger(value) || (value as number) < 0 || (value as number) > TASK_RUNTIME_LIMITS.maxPtyId) {
        throw new TaskBackendProtocolError();
    }
    return value as number;
}

function requireCompletion(value: unknown): Promise<TaskProcessExit> {
    if ((typeof value !== "object" && typeof value !== "function") || value === null) throw new TaskBackendProtocolError();
    let then: unknown;
    try {
        then = (value as Partial<PromiseLike<TaskProcessExit>>).then;
    } catch {
        throw new TaskBackendProtocolError();
    }
    if (typeof then !== "function") throw new TaskBackendProtocolError();
    return Promise.resolve(value as PromiseLike<TaskProcessExit>);
}

function normalizeExit(value: unknown): TaskProcessExit {
    try {
        if (typeof value !== "object" || value === null) throw new TaskBackendProtocolError();
        const code = (value as Partial<TaskProcessExit>).code;
        const rawSignal = (value as Partial<TaskProcessExit>).signal;
        if (!Number.isSafeInteger(code)) throw new TaskBackendProtocolError();
        if (rawSignal !== undefined && rawSignal !== null) {
            if (
                typeof rawSignal !== "string" ||
                rawSignal.length === 0 ||
                rawSignal.length > TASK_RUNTIME_LIMITS.maxSignalLength ||
                containsControlCharacter(rawSignal)
            ) {
                throw new TaskBackendProtocolError();
            }
        }
        return Object.freeze({ code: code as number, signal: rawSignal ?? null });
    } catch (error) {
        if (error instanceof TaskBackendProtocolError) throw error;
        throw new TaskBackendProtocolError();
    }
}

function copyEnvironment(environment: Readonly<Record<string, string>>): Readonly<Record<string, string>> {
    return Object.freeze(Object.fromEntries(Object.entries(environment)));
}

function terminalKey(task: ResolvedTaskDefinition): string {
    return JSON.stringify(["task", task.project, task.id]);
}

/**
 * Converts trusted task definitions into independently owned headless PTYs.
 *
 * Final app wiring seam: add a native task-spawn command that accepts this
 * structured request (including arbitrary validated env), returns an exact PTY
 * ID plus task-process exit observation, and keeps a bounded attach snapshot.
 * Add a state command that opens/reuses a command terminal bound to that
 * existing PTY ID and implement TaskTerminalSurface with it. Do not route this
 * through `runCustomCommand`: it drops task env and creates a second,
 * renderer-owned PTY whose lifetime cannot safely stop the task process.
 */
export class HeadlessPtyTaskRunner implements TaskRunner {
    private readonly startTask: (request: TaskExecutionRequest) => TaskExecutionStart | PromiseLike<TaskExecutionStart>;
    private readonly stopTask: (ptyId: number) => void | PromiseLike<void>;
    private readonly openTerminal: (request: TaskTerminalOpenRequest) => void | PromiseLike<void>;
    private readonly cols: number;
    private readonly rows: number;
    private sequence = 0;

    constructor(options: HeadlessPtyTaskRunnerOptions) {
        if (typeof options !== "object" || options === null) throw new TypeError("task runner options must be an object");
        if (typeof options.backend !== "object" || options.backend === null) throw new TypeError("task backend must be an object");
        if (typeof options.surface !== "object" || options.surface === null) throw new TypeError("task terminal surface must be an object");
        const start = options.backend.start;
        const stop = options.backend.stop;
        const open = options.surface.open;
        if (typeof start !== "function" || typeof stop !== "function") throw new TypeError("task backend must provide start and stop functions");
        if (typeof open !== "function") throw new TypeError("task terminal surface must provide an open function");
        this.startTask = (request) => Reflect.apply(start, options.backend, [request]) as TaskExecutionStart | PromiseLike<TaskExecutionStart>;
        this.stopTask = (ptyId) => Reflect.apply(stop, options.backend, [ptyId]) as void | PromiseLike<void>;
        this.openTerminal = (request) => Reflect.apply(open, options.surface, [request]) as void | PromiseLike<void>;
        this.cols = requirePositiveInteger("task terminal columns", options.cols, TASK_RUNTIME_LIMITS.defaultColumns, TASK_RUNTIME_LIMITS.maxColumns);
        this.rows = requirePositiveInteger("task terminal rows", options.rows, TASK_RUNTIME_LIMITS.defaultRows, TASK_RUNTIME_LIMITS.maxRows);
    }

    async run(task: ResolvedTaskDefinition): Promise<TaskRunnerHandle> {
        if (this.sequence >= Number.MAX_SAFE_INTEGER) throw new RangeError("task execution ID space exhausted");
        this.sequence += 1;
        const stableTerminalKey = terminalKey(task);
        const request: TaskExecutionRequest = Object.freeze({
            executionId: JSON.stringify([stableTerminalKey, this.sequence]),
            terminalKey: stableTerminalKey,
            taskId: task.id,
            label: task.label,
            project: task.project,
            source: task.source,
            command: task.command,
            cwd: task.cwd,
            env: copyEnvironment(task.env),
            cols: this.cols,
            rows: this.rows,
        });

        let started: TaskExecutionStart;
        try {
            started = await callAsPromise(() => this.startTask(request));
        } catch {
            throw new TaskBackendStartError();
        }

        let ptyId: number;
        try {
            ptyId = requirePtyId(started?.ptyId);
        } catch {
            throw new TaskBackendProtocolError();
        }

        let processCompletion: Promise<TaskProcessExit>;
        try {
            processCompletion = requireCompletion(started.completion);
        } catch {
            const cleanupFailed = await this.cleanupInvalidStart(ptyId);
            throw new TaskBackendProtocolError(cleanupFailed);
        }

        return this.createHandle(request, ptyId, processCompletion);
    }

    /** A run the core kept going while no page watched it; its terminal opens without taking focus. */
    adopt(task: ResolvedTaskDefinition, executionId: string, started: TaskExecutionStart): TaskRunnerHandle {
        const stableTerminalKey = terminalKey(task);
        const request: TaskExecutionRequest = Object.freeze({
            executionId,
            terminalKey: stableTerminalKey,
            taskId: task.id,
            label: task.label,
            project: task.project,
            source: task.source,
            command: task.command,
            cwd: task.cwd,
            env: copyEnvironment(task.env),
            cols: this.cols,
            rows: this.rows,
        });
        return this.createHandle(request, requirePtyId(started.ptyId), requireCompletion(started.completion), true);
    }

    private createHandle(
        request: TaskExecutionRequest,
        ptyId: number,
        processCompletion: Promise<TaskProcessExit>,
        background = false,
    ): TaskRunnerHandle {
        const abort = new AbortController();
        let state: RunState = "active";
        let surfaceOpened = false;
        let pendingProcessResult: PendingProcessResult | null = null;
        let stopPromise: Promise<void> | null = null;
        let resolveCompletion!: () => void;
        let rejectCompletion!: (error: unknown) => void;
        const completion = containRejection(
            new Promise<void>((resolve, reject) => {
                resolveCompletion = resolve;
                rejectCompletion = reject;
            }),
        );

        const ensureStop = (): Promise<void> => {
            if (stopPromise) return stopPromise;
            const attempt = callAsPromise(() => this.stopTask(ptyId)).catch(() => {
                throw new TaskBackendStopError();
            });
            stopPromise = containRejection(attempt);
            return stopPromise;
        };

        const stop = (): Promise<void> => {
            if (state === "process-settled") {
                state = "external-stop";
                pendingProcessResult = null;
                abort.abort();
                resolveCompletion();
                return RESOLVED_VOID;
            }
            if (state === "active") {
                state = "external-stop";
                abort.abort();
                void ensureStop().then(resolveCompletion, rejectCompletion);
            }
            return ensureStop();
        };

        const failBackendObservation = (error: TaskBackendCompletionError | TaskBackendProtocolError): void => {
            if (state !== "active") return;
            state = "backend-failure";
            abort.abort();
            void ensureStop().then(
                () => rejectCompletion(error),
                (stopError) => rejectCompletion(stopError),
            );
        };

        const settleProcessAfterPresentation = (): void => {
            const result = pendingProcessResult;
            if (!surfaceOpened || state !== "process-settled" || !result) return;
            pendingProcessResult = null;
            if (result.error) rejectCompletion(result.error);
            else resolveCompletion();
        };

        void processCompletion.then(
            (rawExit) => {
                if (state !== "active") return;
                let exit: TaskProcessExit;
                try {
                    exit = normalizeExit(rawExit);
                } catch {
                    failBackendObservation(new TaskBackendProtocolError());
                    return;
                }
                state = "process-settled";
                pendingProcessResult = Object.freeze({ error: exit.code === 0 ? null : new TaskProcessExitError(exit.code, exit.signal ?? null) });
                settleProcessAfterPresentation();
            },
            () => {
                failBackendObservation(new TaskBackendCompletionError());
            },
        );

        const surfaceRequest: TaskTerminalOpenRequest = Object.freeze({
            executionId: request.executionId,
            terminalKey: request.terminalKey,
            ptyId,
            taskId: request.taskId,
            label: request.label,
            project: request.project,
            source: request.source,
            cwd: request.cwd,
            ...(background ? { background } : {}),
            signal: abort.signal,
        });
        void callAsPromise(() => this.openTerminal(surfaceRequest)).then(
            () => {
                surfaceOpened = true;
                settleProcessAfterPresentation();
            },
            () => {
                if (state !== "active" && state !== "process-settled") return;
                state = "surface-failure";
                pendingProcessResult = null;
                abort.abort();
                void ensureStop().then(
                    () => rejectCompletion(new TaskTerminalSurfaceError()),
                    () => rejectCompletion(new TaskTerminalSurfaceError(true)),
                );
            },
        );

        return Object.freeze({ completion, stop });
    }

    private async cleanupInvalidStart(ptyId: number): Promise<boolean> {
        try {
            await callAsPromise(() => this.stopTask(ptyId));
            return false;
        } catch {
            // The protocol error remains stable and secret-free. The backend's
            // failed exact-ID cleanup is intentionally not retained.
            return true;
        }
    }
}

/** Bounded controller owner with one independently stoppable task per project. */
export class TaskRuntime {
    private readonly registry: TaskRegistry;
    private readonly runner: HeadlessPtyTaskRunner;
    private readonly controllerOptions: TaskControllerOptions;
    private readonly maxProjectControllers: number;
    /** In least-recently-used to most-recently-used order. */
    private readonly controllers = new Map<string, TaskController>();
    private disposed = false;
    private disposePromise: Promise<void> | null = null;

    constructor(options: TaskRuntimeOptions) {
        if (typeof options !== "object" || options === null) throw new TypeError("task runtime options must be an object");
        if (typeof options.registry !== "object" || options.registry === null) throw new TypeError("task registry must be an object");
        this.registry = options.registry;
        this.runner = new HeadlessPtyTaskRunner(options);
        this.controllerOptions = Object.freeze({ ...(options.controllerOptions ?? {}) });
        this.maxProjectControllers = requirePositiveInteger(
            "maxProjectControllers",
            options.maxProjectControllers,
            TASK_RUNTIME_LIMITS.defaultProjectControllers,
            TASK_RUNTIME_LIMITS.maxProjectControllers,
        );
    }

    run(projectInput: string, taskId: string): Promise<void> {
        if (this.disposed) return rejected(new TaskRuntimeDisposedError());
        let project: string;
        let task: ResolvedTaskDefinition | undefined;
        let controller: TaskController;
        try {
            project = requireProject(projectInput);
            task = this.registry.get(project, taskId);
            if (!task) throw new TaskRuntimeTaskNotFoundError();
            controller = this.controllerFor(project);
        } catch (error) {
            return rejected(error);
        }
        return this.trackRecent(controller.run(task), task);
    }

    /** Shows a task the core kept running as this project's running task, so it can be stopped and restarted. */
    adopt(task: ResolvedTaskDefinition, executionId: string, started: TaskExecutionStart): Promise<void> {
        if (this.disposed) return rejected(new TaskRuntimeDisposedError());
        let controller: TaskController;
        try {
            controller = this.controllerFor(requireProject(task.project));
        } catch (error) {
            return rejected(error);
        }
        return controller.adopt(task, () => this.runner.adopt(task, executionId, started));
    }

    restart(projectInput: string, taskId?: string): Promise<void> {
        if (this.disposed) return rejected(new TaskRuntimeDisposedError());
        let project: string;
        try {
            project = requireProject(projectInput);
            if (taskId !== undefined) {
                const task = this.registry.get(project, taskId);
                if (!task) throw new TaskRuntimeTaskNotFoundError();
                return this.trackRecent(this.controllerFor(project).restart(task), task);
            }
            const controller = this.controllers.get(project);
            if (!controller) throw new TaskUnavailableError();
            this.touch(project, controller);
            const task = controller.getSnapshot().task;
            return task ? this.trackRecent(controller.restart(), task) : controller.restart();
        } catch (error) {
            return rejected(error);
        }
    }

    stop(projectInput: string): Promise<void> {
        if (this.disposed) return rejected(new TaskRuntimeDisposedError());
        let project: string;
        try {
            project = requireProject(projectInput);
        } catch (error) {
            return rejected(error);
        }
        const controller = this.controllers.get(project);
        if (!controller) return RESOLVED_VOID;
        this.touch(project, controller);
        return controller.stop();
    }

    getSnapshot(projectInput: string): TaskControllerSnapshot | null {
        this.assertActive();
        const project = requireProject(projectInput);
        const controller = this.controllers.get(project);
        if (!controller) return null;
        this.touch(project, controller);
        return controller.getSnapshot();
    }

    disposeProject(projectInput: string): Promise<void> {
        if (this.disposed) return rejected(new TaskRuntimeDisposedError());
        let project: string;
        try {
            project = requireProject(projectInput);
        } catch (error) {
            return rejected(error);
        }
        const controller = this.controllers.get(project);
        if (!controller) return RESOLVED_VOID;
        this.controllers.delete(project);
        return controller.dispose();
    }

    dispose(): Promise<void> {
        if (this.disposePromise) return this.disposePromise;
        this.disposed = true;
        const controllers = [...this.controllers.values()];
        this.controllers.clear();
        const attempt = Promise.allSettled(controllers.map((controller) => controller.dispose())).then((results) => {
            if (results.some((result) => result.status === "rejected")) throw new TaskRuntimeDisposeError();
        });
        this.disposePromise = containRejection(attempt);
        return this.disposePromise;
    }

    private controllerFor(project: string): TaskController {
        const existing = this.controllers.get(project);
        if (existing) {
            this.touch(project, existing);
            return existing;
        }
        if (this.controllers.size >= this.maxProjectControllers) {
            const idle = [...this.controllers].find(([, controller]) => {
                const snapshot = controller.getSnapshot();
                return snapshot.activeRunId === null && snapshot.status !== "running" && snapshot.status !== "stopping";
            });
            if (!idle) throw new TaskRuntimeCapacityError();
            this.controllers.delete(idle[0]);
            void idle[1].dispose();
        }
        const controller = new TaskController(this.runner, this.controllerOptions);
        this.controllers.set(project, controller);
        return controller;
    }

    private touch(project: string, controller: TaskController): void {
        this.controllers.delete(project);
        this.controllers.set(project, controller);
    }

    private trackRecent(started: Promise<void>, task: ResolvedTaskDefinition): Promise<void> {
        const tracked = started.then(() => {
            if (this.disposed) return;
            try {
                this.registry.rememberRecent(task);
            } catch {
                // A registry may be independently disposed after a task starts.
                // That must never orphan or misreport the live process.
            }
        });
        return containRejection(tracked);
    }

    private assertActive(): void {
        if (this.disposed) throw new TaskRuntimeDisposedError();
    }
}
