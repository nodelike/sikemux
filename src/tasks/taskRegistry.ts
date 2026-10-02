export type TaskSource = "built-in" | "project" | "recent";
export type TaskLifecycleState = "idle" | "running" | "stopping" | "failed";
export type TaskFailureOperation = "run" | "completion" | "stop";

/** Highest-precedence source first. Recent entries are fallback snapshots. */
export const TASK_SOURCE_PRECEDENCE = Object.freeze(["project", "built-in", "recent"] as const);

export const TASK_REGISTRY_LIMITS = Object.freeze({
    maxTasksPerSource: 2_048,
    maxRecentTasks: 64,
    maxIdLength: 128,
    maxLabelLength: 256,
    maxProjectLength: 4_096,
    maxCommandLength: 16_384,
    maxCwdLength: 4_096,
    maxEnvEntries: 128,
    maxEnvKeyLength: 256,
    maxEnvValueLength: 8_192,
    maxEnvTotalLength: 65_536,
    maxListeners: 128,
    maxFailureHistory: 64,
    maxFailureMessageLength: 512,
});

export interface TaskDefinitionInput {
    readonly id: string;
    readonly label: string;
    readonly project: string;
    readonly command: string;
    readonly cwd: string;
    readonly env?: Readonly<Record<string, string>>;
}

export interface TaskDefinition {
    readonly id: string;
    readonly label: string;
    readonly project: string;
    readonly command: string;
    readonly cwd: string;
    readonly env: Readonly<Record<string, string>>;
}

export interface ResolvedTaskDefinition extends TaskDefinition {
    readonly source: TaskSource;
}

export interface TaskRegistrySnapshot {
    readonly revision: number;
    readonly disposed: boolean;
    readonly tasks: readonly ResolvedTaskDefinition[];
    /** Most-recently used first, including entries shadowed by fresher sources. */
    readonly recent: readonly ResolvedTaskDefinition[];
    readonly sourceCounts: Readonly<Record<TaskSource, number>>;
}

export interface TaskRegistryOptions {
    readonly maxTasksPerSource?: number;
    readonly maxRecentTasks?: number;
    readonly maxListeners?: number;
}

export interface TaskRunnerHandle {
    readonly completion: PromiseLike<void>;
    stop(): void | PromiseLike<void>;
}

export interface TaskRunner {
    run(task: ResolvedTaskDefinition): TaskRunnerHandle | PromiseLike<TaskRunnerHandle>;
}

export interface TaskFailure {
    readonly sequence: number;
    readonly at: number;
    readonly runId: number;
    readonly taskId: string;
    readonly project: string;
    readonly operation: TaskFailureOperation;
    readonly message: string;
}

export interface TaskControllerSnapshot {
    readonly revision: number;
    readonly status: TaskLifecycleState;
    readonly disposed: boolean;
    /** The active task, or the last task after it settles so restart remains available. */
    readonly task: ResolvedTaskDefinition | null;
    readonly activeRunId: number | null;
    readonly runAttempts: number;
    readonly failures: readonly TaskFailure[];
}

export interface TaskControllerOptions {
    readonly maxFailureHistory?: number;
    readonly maxListeners?: number;
    readonly now?: () => number;
}

export class DuplicateTaskDefinitionError extends Error {
    constructor(
        readonly source: TaskSource,
        readonly project: string,
        readonly taskId: string,
    ) {
        super(`Duplicate ${source} task: ${project}/${taskId}`);
        this.name = "DuplicateTaskDefinitionError";
    }
}

export class TaskRegistryDisposedError extends Error {
    constructor() {
        super("Task registry has been disposed");
        this.name = "TaskRegistryDisposedError";
    }
}

export class TaskControllerDisposedError extends Error {
    constructor() {
        super("Task controller has been disposed");
        this.name = "TaskControllerDisposedError";
    }
}

export class TaskAlreadyRunningError extends Error {
    constructor() {
        super("A task run or restart is already in progress");
        this.name = "TaskAlreadyRunningError";
    }
}

export class TaskUnavailableError extends Error {
    constructor() {
        super("No task is available to restart");
        this.name = "TaskUnavailableError";
    }
}

type RegistryListener = () => void;
type ControllerListener = () => void;

interface NormalizedTaskRunnerHandle {
    readonly completion: Promise<void>;
    readonly stop: () => Promise<void>;
}

interface ActiveTaskRun {
    readonly id: number;
    readonly generation: number;
    readonly task: ResolvedTaskDefinition;
    readonly handle: Promise<NormalizedTaskRunnerHandle>;
    stopPromise: Promise<void> | null;
}

const TASK_SOURCES = new Set<TaskSource>(TASK_SOURCE_PRECEDENCE);
const UNSAFE_RECORD_KEYS = new Set(["__proto__", "constructor", "prototype"]);
const TASK_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]*$/u;
const RESOLVED_VOID = Promise.resolve();

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function containsNul(value: string): boolean {
    return value.includes("\0");
}

function requireTaskSource(value: unknown): TaskSource {
    if (typeof value !== "string" || !TASK_SOURCES.has(value as TaskSource)) {
        throw new TypeError("task source must be built-in, project, or recent");
    }
    return value as TaskSource;
}

function requireTaskId(value: unknown): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > TASK_REGISTRY_LIMITS.maxIdLength ||
        value !== value.trim() ||
        containsControlCharacter(value) ||
        UNSAFE_RECORD_KEYS.has(value) ||
        !TASK_ID_PATTERN.test(value)
    ) {
        throw new TypeError("task id must be a bounded, trimmed, record-safe identifier");
    }
    return value;
}

function requireLabel(value: unknown): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > TASK_REGISTRY_LIMITS.maxLabelLength ||
        value !== value.trim() ||
        containsControlCharacter(value)
    ) {
        throw new TypeError("task label must be bounded, trimmed text without control characters");
    }
    return value;
}

function requireNonBlankScalar(name: string, value: unknown, maxLength: number, allowControls = false): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > maxLength ||
        value.trim().length === 0 ||
        containsNul(value) ||
        (!allowControls && containsControlCharacter(value))
    ) {
        throw new TypeError(`${name} must be a bounded, non-blank scalar string without disallowed controls`);
    }
    return value;
}

function requireEnvironmentKey(value: string): string {
    if (
        value.length === 0 ||
        value.length > TASK_REGISTRY_LIMITS.maxEnvKeyLength ||
        value.includes("=") ||
        containsControlCharacter(value) ||
        UNSAFE_RECORD_KEYS.has(value)
    ) {
        throw new TypeError("task environment keys must be bounded, record-safe strings without controls or equals signs");
    }
    return value;
}

function copyEnvironment(value: unknown): Readonly<Record<string, string>> {
    if (value === undefined) return Object.freeze({});
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new TypeError("task env must be a record of scalar strings");
    }

    const keys = Reflect.ownKeys(value);
    if (keys.length > TASK_REGISTRY_LIMITS.maxEnvEntries) {
        throw new RangeError(`task env cannot contain more than ${TASK_REGISTRY_LIMITS.maxEnvEntries} entries`);
    }

    let totalLength = 0;
    const entries: Array<readonly [string, string]> = [];
    for (const key of keys) {
        if (typeof key !== "string") throw new TypeError("task env cannot contain symbol keys");
        const descriptor = Object.getOwnPropertyDescriptor(value, key);
        if (!descriptor || !descriptor.enumerable || !("value" in descriptor) || typeof descriptor.value !== "string") {
            throw new TypeError("task env must contain only enumerable scalar string values");
        }
        const environmentKey = requireEnvironmentKey(key);
        const environmentValue = descriptor.value;
        if (environmentValue.length > TASK_REGISTRY_LIMITS.maxEnvValueLength || containsNul(environmentValue)) {
            throw new TypeError("task environment values must be bounded strings without NUL bytes");
        }
        totalLength += environmentKey.length + environmentValue.length;
        if (totalLength > TASK_REGISTRY_LIMITS.maxEnvTotalLength) {
            throw new RangeError(`task env cannot exceed ${TASK_REGISTRY_LIMITS.maxEnvTotalLength} total characters`);
        }
        entries.push([environmentKey, environmentValue]);
    }
    entries.sort(([left], [right]) => compareStrings(left, right));
    return Object.freeze(Object.fromEntries(entries));
}

export function createTaskDefinition(input: TaskDefinitionInput): TaskDefinition {
    if (typeof input !== "object" || input === null) throw new TypeError("task definition must be an object");
    return Object.freeze({
        id: requireTaskId(input.id),
        label: requireLabel(input.label),
        project: requireNonBlankScalar("task project", input.project, TASK_REGISTRY_LIMITS.maxProjectLength),
        command: requireNonBlankScalar("task command", input.command, TASK_REGISTRY_LIMITS.maxCommandLength, true),
        cwd: requireNonBlankScalar("task cwd", input.cwd, TASK_REGISTRY_LIMITS.maxCwdLength),
        env: copyEnvironment(input.env),
    });
}

function createResolvedTaskDefinition(input: TaskDefinitionInput, source: TaskSource): ResolvedTaskDefinition {
    const definition = createTaskDefinition(input);
    return Object.freeze({ ...definition, source: requireTaskSource(source) });
}

function copyResolvedTaskDefinition(input: ResolvedTaskDefinition): ResolvedTaskDefinition {
    if (typeof input !== "object" || input === null) throw new TypeError("resolved task definition must be an object");
    return createResolvedTaskDefinition(input, requireTaskSource(input.source));
}

function taskKey(project: string, taskId: string): string {
    return JSON.stringify([project, taskId]);
}

function compareStrings(left: string, right: string): number {
    return left < right ? -1 : left > right ? 1 : 0;
}

function compareTasks(left: ResolvedTaskDefinition, right: ResolvedTaskDefinition): number {
    return (
        compareStrings(left.project, right.project) ||
        compareStrings(left.label, right.label) ||
        compareStrings(left.id, right.id) ||
        TASK_SOURCE_PRECEDENCE.indexOf(left.source) - TASK_SOURCE_PRECEDENCE.indexOf(right.source)
    );
}

function requireLimit(name: string, value: number | undefined, fallback: number, hardLimit: number): number {
    const resolved = value ?? fallback;
    if (!Number.isSafeInteger(resolved) || resolved <= 0 || resolved > hardLimit) {
        throw new RangeError(`${name} must be a positive integer no greater than ${hardLimit}`);
    }
    return resolved;
}

function callAsPromise<T>(operation: () => T | PromiseLike<T>): Promise<T> {
    try {
        return Promise.resolve(operation());
    } catch (error) {
        return Promise.reject(error);
    }
}

/** Preserve the returned promise while making fire-and-forget use rejection-safe. */
function containRejection<T>(promise: Promise<T>): Promise<T> {
    void promise.catch(() => {});
    return promise;
}

function rejected<T>(error: unknown): Promise<T> {
    return containRejection(Promise.reject(error));
}

function normalizeRunnerHandle(value: unknown): NormalizedTaskRunnerHandle {
    if ((typeof value !== "object" && typeof value !== "function") || value === null) {
        throw new TypeError("task runner returned an invalid handle");
    }
    const candidate = value as Partial<TaskRunnerHandle>;
    const stop = candidate.stop;
    const completion = candidate.completion;
    if (typeof stop !== "function" || (typeof completion !== "object" && typeof completion !== "function") || completion === null) {
        throw new TypeError("task runner handle must provide stop and completion");
    }
    const then = (completion as Partial<PromiseLike<void>>).then;
    if (typeof then !== "function") throw new TypeError("task runner handle completion must be promise-like");
    const completionPromise = Promise.resolve(completion as PromiseLike<void>);
    // A handle may resolve after its run was stopped or disposed. Its eventual
    // completion is intentionally stale, but its rejection must still be owned.
    void completionPromise.catch(() => {});
    return Object.freeze({
        completion: completionPromise,
        stop: () => callAsPromise(() => Reflect.apply(stop, value, [])),
    });
}

function errorMessage(error: unknown): string {
    let raw = "Task operation failed";
    if (typeof error === "string") {
        raw = error;
    } else if ((typeof error === "object" || typeof error === "function") && error !== null) {
        try {
            const descriptor = Object.getOwnPropertyDescriptor(error, "message");
            if (descriptor && "value" in descriptor && typeof descriptor.value === "string") raw = descriptor.value;
        } catch {
            // Never enumerate or stringify opaque runner errors.
        }
    }

    const maxLength = TASK_REGISTRY_LIMITS.maxFailureMessageLength;
    const scanLimit = Math.min(raw.length, maxLength * 8);
    let sanitized = "";
    let pendingSpace = false;
    for (let index = 0; index < scanLimit;) {
        const code = raw.codePointAt(index) ?? 0;
        const character = String.fromCodePoint(code);
        index += character.length;

        if (code === 27 && raw[index] === "[") {
            index += 1;
            while (index < scanLimit) {
                const control = raw.charCodeAt(index);
                index += 1;
                if (control >= 64 && control <= 126) break;
            }
            pendingSpace = sanitized.length > 0;
            continue;
        }
        if (code === 27 && raw[index] === "]") {
            index += 1;
            while (index < scanLimit) {
                const control = raw.charCodeAt(index);
                index += 1;
                if (control === 7) break;
                if (control === 27 && raw.charCodeAt(index) === 92) {
                    index += 1;
                    break;
                }
            }
            pendingSpace = sanitized.length > 0;
            continue;
        }
        if (code <= 31 || (code >= 127 && code <= 159) || /\s/u.test(character)) {
            pendingSpace = sanitized.length > 0;
            continue;
        }
        if (pendingSpace) {
            sanitized += " ";
            pendingSpace = false;
        }
        if (sanitized.length + character.length > maxLength) break;
        sanitized += character;
        if (sanitized.length >= maxLength) break;
    }
    sanitized = sanitized.trim();
    return sanitized || "Task operation failed";
}

/**
 * Framework-independent task inventory. Source replacement is atomic, project
 * definitions override built-ins, and recent definitions are bounded fallbacks.
 */
export class TaskRegistry {
    private readonly sourceTasks = new Map<TaskSource, Map<string, ResolvedTaskDefinition>>([
        ["built-in", new Map()],
        ["project", new Map()],
        ["recent", new Map()],
    ]);
    private readonly listeners = new Set<RegistryListener>();
    private readonly maxTasksPerSource: number;
    private readonly maxRecentTasks: number;
    private readonly maxListeners: number;
    private recentOrder: string[] = [];
    private revision = 0;
    private disposed = false;
    private snapshotValue: TaskRegistrySnapshot;

    constructor(options: TaskRegistryOptions = {}) {
        this.maxTasksPerSource = requireLimit(
            "maxTasksPerSource",
            options.maxTasksPerSource,
            TASK_REGISTRY_LIMITS.maxTasksPerSource,
            TASK_REGISTRY_LIMITS.maxTasksPerSource,
        );
        this.maxRecentTasks = requireLimit(
            "maxRecentTasks",
            options.maxRecentTasks,
            TASK_REGISTRY_LIMITS.maxRecentTasks,
            TASK_REGISTRY_LIMITS.maxRecentTasks,
        );
        this.maxListeners = requireLimit("maxListeners", options.maxListeners, TASK_REGISTRY_LIMITS.maxListeners, TASK_REGISTRY_LIMITS.maxListeners);
        this.snapshotValue = this.buildSnapshot();
    }

    getSnapshot(): TaskRegistrySnapshot {
        return this.snapshotValue;
    }

    get(project: string, taskId: string): ResolvedTaskDefinition | undefined {
        const key = taskKey(requireNonBlankScalar("task project", project, TASK_REGISTRY_LIMITS.maxProjectLength), requireTaskId(taskId));
        for (const source of TASK_SOURCE_PRECEDENCE) {
            const task = this.tasksFor(source).get(key);
            if (task) return task;
        }
        return undefined;
    }

    list(project?: string): readonly ResolvedTaskDefinition[] {
        if (project === undefined) return this.snapshotValue.tasks;
        const validProject = requireNonBlankScalar("task project", project, TASK_REGISTRY_LIMITS.maxProjectLength);
        return Object.freeze(this.snapshotValue.tasks.filter((task) => task.project === validProject));
    }

    /**
     * Atomically replace one source. For `recent`, input order is most-recent
     * first and must already fit the configured recent bound.
     */
    replaceSource(sourceInput: TaskSource, definitions: Iterable<TaskDefinitionInput>): void {
        this.assertActive();
        const source = requireTaskSource(sourceInput);
        const limit = source === "recent" ? this.maxRecentTasks : this.maxTasksPerSource;
        const next = new Map<string, ResolvedTaskDefinition>();
        const order: string[] = [];
        let count = 0;
        for (const input of definitions) {
            count += 1;
            if (count > limit) throw new RangeError(`${source} task source cannot contain more than ${limit} tasks`);
            const task = createResolvedTaskDefinition(input, source);
            const key = taskKey(task.project, task.id);
            if (next.has(key)) throw new DuplicateTaskDefinitionError(source, task.project, task.id);
            next.set(key, task);
            order.push(key);
        }

        this.sourceTasks.set(source, next);
        if (source === "recent") this.recentOrder = order;
        this.publish();
    }

    /** Promote a trusted task snapshot to MRU and evict the least-recent entry. */
    rememberRecent(input: TaskDefinitionInput): ResolvedTaskDefinition {
        this.assertActive();
        const task = createResolvedTaskDefinition(input, "recent");
        const key = taskKey(task.project, task.id);
        const recent = this.tasksFor("recent");
        recent.set(key, task);
        this.recentOrder = [key, ...this.recentOrder.filter((candidate) => candidate !== key)];
        while (this.recentOrder.length > this.maxRecentTasks) {
            const evicted = this.recentOrder.pop();
            if (evicted !== undefined) recent.delete(evicted);
        }
        this.publish();
        return task;
    }

    subscribe(listener: RegistryListener): () => void {
        this.assertActive();
        if (typeof listener !== "function") throw new TypeError("task registry listener must be a function");
        if (this.listeners.size >= this.maxListeners) throw new RangeError("task registry listener limit reached");
        this.listeners.add(listener);
        let subscribed = true;
        return () => {
            if (!subscribed) return;
            subscribed = false;
            this.listeners.delete(listener);
        };
    }

    dispose(): void {
        if (this.disposed) return;
        this.disposed = true;
        for (const source of TASK_SOURCE_PRECEDENCE) this.tasksFor(source).clear();
        this.recentOrder = [];
        this.publish();
        this.listeners.clear();
    }

    private tasksFor(source: TaskSource): Map<string, ResolvedTaskDefinition> {
        return this.sourceTasks.get(source)!;
    }

    private assertActive(): void {
        if (this.disposed) throw new TaskRegistryDisposedError();
    }

    private publish(): void {
        this.revision += 1;
        this.snapshotValue = this.buildSnapshot();
        for (const listener of this.listeners) {
            try {
                listener();
            } catch {
                this.listeners.delete(listener);
            }
        }
    }

    private buildSnapshot(): TaskRegistrySnapshot {
        const keys = new Set<string>();
        for (const source of TASK_SOURCE_PRECEDENCE) {
            for (const key of this.tasksFor(source).keys()) keys.add(key);
        }
        const tasks: ResolvedTaskDefinition[] = [];
        for (const key of keys) {
            for (const source of TASK_SOURCE_PRECEDENCE) {
                const task = this.tasksFor(source).get(key);
                if (task) {
                    tasks.push(task);
                    break;
                }
            }
        }
        tasks.sort(compareTasks);

        const recent = this.tasksFor("recent");
        const recentTasks = this.recentOrder.flatMap((key) => {
            const task = recent.get(key);
            return task ? [task] : [];
        });
        return Object.freeze({
            revision: this.revision,
            disposed: this.disposed,
            tasks: Object.freeze(tasks),
            recent: Object.freeze(recentTasks),
            sourceCounts: Object.freeze({
                "built-in": this.tasksFor("built-in").size,
                project: this.tasksFor("project").size,
                recent: recent.size,
            }),
        });
    }
}

/** Owns at most one injected runner handle and exposes immutable lifecycle state. */
export class TaskController {
    private readonly runTask: (task: ResolvedTaskDefinition) => TaskRunnerHandle | PromiseLike<TaskRunnerHandle>;
    private readonly listeners = new Set<ControllerListener>();
    private readonly maxFailureHistory: number;
    private readonly maxListeners: number;
    private readonly now: () => number;
    private readonly failures: TaskFailure[] = [];
    private snapshotValue: TaskControllerSnapshot;
    private status: TaskLifecycleState = "idle";
    private task: ResolvedTaskDefinition | null = null;
    private active: ActiveTaskRun | null = null;
    private activeRunId: number | null = null;
    private restartPromise: Promise<void> | null = null;
    private disposePromise: Promise<void> | null = null;
    private generation = 0;
    private runSequence = 0;
    private failureSequence = 0;
    private revision = 0;
    private runAttempts = 0;
    private disposed = false;

    constructor(runner: TaskRunner, options: TaskControllerOptions = {}) {
        if (typeof runner !== "object" || runner === null || typeof runner.run !== "function") {
            throw new TypeError("task runner must provide a run function");
        }
        const run = runner.run;
        this.runTask = (task) => Reflect.apply(run, runner, [task]) as TaskRunnerHandle | PromiseLike<TaskRunnerHandle>;
        this.maxFailureHistory = requireLimit(
            "maxFailureHistory",
            options.maxFailureHistory,
            TASK_REGISTRY_LIMITS.maxFailureHistory,
            TASK_REGISTRY_LIMITS.maxFailureHistory,
        );
        this.maxListeners = requireLimit("maxListeners", options.maxListeners, TASK_REGISTRY_LIMITS.maxListeners, TASK_REGISTRY_LIMITS.maxListeners);
        if (options.now !== undefined && typeof options.now !== "function") throw new TypeError("task controller clock must be a function");
        this.now = options.now ?? Date.now;
        this.snapshotValue = this.buildSnapshot();
    }

    getSnapshot(): TaskControllerSnapshot {
        return this.snapshotValue;
    }

    subscribe(listener: ControllerListener): () => void {
        if (this.disposed) throw new TaskControllerDisposedError();
        if (typeof listener !== "function") throw new TypeError("task controller listener must be a function");
        if (this.listeners.size >= this.maxListeners) throw new RangeError("task controller listener limit reached");
        this.listeners.add(listener);
        let subscribed = true;
        return () => {
            if (!subscribed) return;
            subscribed = false;
            this.listeners.delete(listener);
        };
    }

    run(input: ResolvedTaskDefinition): Promise<void> {
        if (this.disposed) return rejected(new TaskControllerDisposedError());
        if (this.active || this.restartPromise) return rejected(new TaskAlreadyRunningError());
        return this.startRun(input);
    }

    /** Takes over a run already going, which `take` hands back without starting anything. */
    adopt(input: ResolvedTaskDefinition, take: () => TaskRunnerHandle | PromiseLike<TaskRunnerHandle>): Promise<void> {
        if (this.disposed) return rejected(new TaskControllerDisposedError());
        if (this.active || this.restartPromise) return rejected(new TaskAlreadyRunningError());
        return this.startRun(input, take);
    }

    restart(input?: ResolvedTaskDefinition): Promise<void> {
        if (this.disposePromise || this.disposed) return rejected(new TaskControllerDisposedError());
        if (this.restartPromise) return this.restartPromise;

        let task: ResolvedTaskDefinition;
        try {
            const candidate = input ?? this.task;
            if (!candidate) return rejected(new TaskUnavailableError());
            task = copyResolvedTaskDefinition(candidate);
        } catch (error) {
            return rejected(error);
        }

        const attempt = this.stop().then(() => {
            if (this.disposed) throw new TaskControllerDisposedError();
            return this.startRun(task);
        });
        const tracked = attempt.finally(() => {
            if (this.restartPromise === tracked) this.restartPromise = null;
        });
        this.restartPromise = containRejection(tracked);
        return this.restartPromise;
    }

    stop(): Promise<void> {
        const run = this.active;
        if (!run) return RESOLVED_VOID;
        if (run.stopPromise) return run.stopPromise;

        try {
            this.generation = this.nextSequence(this.generation, "task generation");
        } catch (error) {
            return rejected(error);
        }
        this.status = "stopping";
        this.publish();

        const attempt = run.handle
            .then(
                (handle) => handle.stop(),
                () => undefined,
            )
            .then(() => {
                if (this.active !== run) return;
                this.active = null;
                this.activeRunId = null;
                this.status = "idle";
                this.publish();
            })
            .catch((error: unknown) => {
                if (this.active === run) {
                    this.recordFailure(run, "stop", error);
                    this.status = "failed";
                    this.publish();
                }
                throw error;
            });
        const tracked = attempt.finally(() => {
            if (run.stopPromise === tracked) run.stopPromise = null;
        });
        run.stopPromise = containRejection(tracked);
        return run.stopPromise;
    }

    dispose(): Promise<void> {
        if (this.disposePromise) return this.disposePromise;
        this.disposed = true;
        this.publish();
        const attempt = this.stop();
        const tracked = attempt.finally(() => {
            this.listeners.clear();
        });
        this.disposePromise = containRejection(tracked);
        return this.disposePromise;
    }

    private startRun(
        input: ResolvedTaskDefinition,
        start: (task: ResolvedTaskDefinition) => TaskRunnerHandle | PromiseLike<TaskRunnerHandle> = this.runTask,
    ): Promise<void> {
        if (this.disposed) return rejected(new TaskControllerDisposedError());
        if (this.active) return rejected(new TaskAlreadyRunningError());

        let task: ResolvedTaskDefinition;
        let runId: number;
        let generation: number;
        try {
            task = copyResolvedTaskDefinition(input);
            runId = this.nextSequence(this.runSequence, "task run ID");
            generation = this.nextSequence(this.generation, "task generation");
        } catch (error) {
            return rejected(error);
        }
        this.runSequence = runId;
        this.generation = generation;
        this.runAttempts += 1;
        this.task = task;
        this.activeRunId = runId;
        this.status = "running";

        const handle = callAsPromise(() => start(task)).then(normalizeRunnerHandle);
        const run: ActiveTaskRun = {
            id: runId,
            generation,
            task,
            handle,
            stopPromise: null,
        };
        this.active = run;
        this.publish();

        const started = handle.then(
            (resolved) => {
                if (this.isCurrent(run)) this.observeCompletion(run, resolved);
            },
            (error: unknown) => {
                if (this.isCurrent(run)) {
                    this.active = null;
                    this.activeRunId = null;
                    this.recordFailure(run, "run", error);
                    this.status = "failed";
                    this.publish();
                }
                throw error;
            },
        );
        return containRejection(started);
    }

    private observeCompletion(run: ActiveTaskRun, handle: NormalizedTaskRunnerHandle): void {
        void handle.completion.then(
            () => {
                if (!this.isCurrent(run)) return;
                this.active = null;
                this.activeRunId = null;
                this.status = "idle";
                this.publish();
            },
            (error: unknown) => {
                if (!this.isCurrent(run)) return;
                this.active = null;
                this.activeRunId = null;
                this.recordFailure(run, "completion", error);
                this.status = "failed";
                this.publish();
            },
        );
    }

    private isCurrent(run: ActiveTaskRun): boolean {
        return !this.disposed && this.active === run && this.generation === run.generation;
    }

    private recordFailure(run: ActiveTaskRun, operation: TaskFailureOperation, error: unknown): void {
        this.failureSequence = this.nextSequence(this.failureSequence, "task failure sequence");
        let at = 0;
        try {
            const value = this.now();
            if (Number.isFinite(value) && value >= 0) at = Math.floor(value);
        } catch {
            // Failure recording must not fail because an injected clock did.
        }
        this.failures.push(
            Object.freeze({
                sequence: this.failureSequence,
                at,
                runId: run.id,
                taskId: run.task.id,
                project: run.task.project,
                operation,
                message: errorMessage(error),
            }),
        );
        if (this.failures.length > this.maxFailureHistory) this.failures.splice(0, this.failures.length - this.maxFailureHistory);
    }

    private nextSequence(value: number, name: string): number {
        if (value >= Number.MAX_SAFE_INTEGER) throw new RangeError(`${name} space exhausted`);
        return value + 1;
    }

    private publish(): void {
        this.revision = this.nextSequence(this.revision, "task controller revision");
        this.snapshotValue = this.buildSnapshot();
        for (const listener of this.listeners) {
            try {
                listener();
            } catch {
                this.listeners.delete(listener);
            }
        }
    }

    private buildSnapshot(): TaskControllerSnapshot {
        return Object.freeze({
            revision: this.revision,
            status: this.status,
            disposed: this.disposed,
            task: this.task,
            activeRunId: this.activeRunId,
            runAttempts: this.runAttempts,
            failures: Object.freeze([...this.failures]),
        });
    }
}
