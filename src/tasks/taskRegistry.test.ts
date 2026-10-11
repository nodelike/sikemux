import { describe, expect, it, vi } from "vitest";
import {
    TASK_REGISTRY_LIMITS,
    TASK_SOURCE_PRECEDENCE,
    DuplicateTaskDefinitionError,
    TaskAlreadyRunningError,
    TaskController,
    TaskControllerDisposedError,
    TaskRegistry,
    TaskRegistryDisposedError,
    TaskUnavailableError,
    createTaskDefinition,
    type ResolvedTaskDefinition,
    type TaskDefinitionInput,
    type TaskRunner,
    type TaskRunnerHandle,
} from "./taskRegistry";

interface Deferred<Value> {
    readonly promise: Promise<Value>;
    readonly resolve: (value: Value | PromiseLike<Value>) => void;
    readonly reject: (error: unknown) => void;
}

function deferred<Value>(): Deferred<Value> {
    let resolve!: (value: Value | PromiseLike<Value>) => void;
    let reject!: (error: unknown) => void;
    const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

function definition(id: string, overrides: Partial<TaskDefinitionInput> = {}): TaskDefinitionInput {
    return {
        id,
        label: `Task ${id}`,
        project: "/workspace/project",
        command: `run ${id}`,
        cwd: "/workspace/project",
        env: { MODE: "test" },
        ...overrides,
    };
}

function resolvedTask(id: string, overrides: Partial<ResolvedTaskDefinition> = {}): ResolvedTaskDefinition {
    return {
        ...definition(id),
        env: { MODE: "test" },
        source: "project",
        ...overrides,
    };
}

function runnerHandle(stopImplementation: () => void | PromiseLike<void> = () => undefined) {
    const completion = deferred<void>();
    const stop = vi.fn(stopImplementation);
    const handle: TaskRunnerHandle = { completion: completion.promise, stop };
    return { completion, stop, handle };
}

async function flushPromises(): Promise<void> {
    await Promise.resolve();
    await Promise.resolve();
}

describe("TaskRegistry definitions", () => {
    it("resolves deterministic project > built-in > recent precedence", () => {
        const registry = new TaskRegistry();
        expect(TASK_SOURCE_PRECEDENCE).toEqual(["project", "built-in", "recent"]);
        registry.replaceSource("recent", [definition("test", { label: "Recent test" })]);
        registry.replaceSource("built-in", [definition("test", { label: "Built-in test" }), definition("zeta", { label: "Zulu" })]);
        registry.replaceSource("project", [definition("test", { label: "Project test" }), definition("alpha", { label: "Alpha" })]);

        expect(registry.get("/workspace/project", "test")?.label).toBe("Project test");
        expect(registry.getSnapshot().sourceCounts).toEqual({ "built-in": 2, project: 2, recent: 1 });
        expect(registry.list().map((task) => `${task.label}:${task.source}`)).toEqual(["Alpha:project", "Project test:project", "Zulu:built-in"]);

        registry.replaceSource("project", []);
        expect(registry.get("/workspace/project", "test")?.label).toBe("Built-in test");
        registry.replaceSource("built-in", []);
        expect(registry.get("/workspace/project", "test")?.label).toBe("Recent test");
    });

    it("replaces sources atomically and enforces duplicate, enumeration, and field caps", () => {
        const registry = new TaskRegistry({ maxTasksPerSource: 2 });
        registry.replaceSource("project", [definition("stable")]);
        const before = registry.getSnapshot();

        expect(() => registry.replaceSource("project", [definition("duplicate"), definition("duplicate")])).toThrow(DuplicateTaskDefinitionError);
        expect(registry.getSnapshot()).toBe(before);
        expect(() => registry.replaceSource("project", [definition("one"), definition("two"), definition("three")])).toThrow(RangeError);
        expect(registry.getSnapshot()).toBe(before);

        const invalidDefinitions: unknown[] = [
            definition("__proto__"),
            definition("bad", { label: "bad\nlabel" }),
            definition("bad", { project: "/bad\0project" }),
            definition("bad", { project: "/bad\nproject" }),
            definition("bad", { command: "   " }),
            definition("bad", { cwd: "/bad\0cwd" }),
            definition("bad", { cwd: "/bad\ncwd" }),
            definition("bad", { env: [] as unknown as Record<string, string> }),
            definition("bad", { env: { VALUE: 42 } as unknown as Record<string, string> }),
            definition("bad", { env: { "BAD=KEY": "value" } }),
        ];
        for (const invalid of invalidDefinitions) {
            expect(() => createTaskDefinition(invalid as TaskDefinitionInput)).toThrow(TypeError);
        }

        const oversizedEnv = Object.fromEntries(
            Array.from({ length: TASK_REGISTRY_LIMITS.maxEnvEntries + 1 }, (_, index) => [`KEY_${index}`, "value"]),
        );
        expect(() => createTaskDefinition(definition("bad", { env: oversizedEnv }))).toThrow(RangeError);
        expect(() => registry.replaceSource("invalid" as "project", [])).toThrow(TypeError);
    });

    it("maintains a bounded recent-task LRU without overriding fresher sources", () => {
        const registry = new TaskRegistry({ maxRecentTasks: 2 });
        registry.rememberRecent(definition("one", { label: "One" }));
        registry.rememberRecent(definition("two", { label: "Two" }));
        registry.rememberRecent(definition("one", { label: "One updated" }));
        expect(registry.getSnapshot().recent.map((task) => task.label)).toEqual(["One updated", "Two"]);

        registry.rememberRecent(definition("three", { label: "Three" }));
        expect(registry.getSnapshot().recent.map((task) => task.id)).toEqual(["three", "one"]);
        expect(registry.get("/workspace/project", "two")).toBeUndefined();

        registry.replaceSource("project", [definition("one", { label: "Current project one" })]);
        expect(registry.get("/workspace/project", "one")?.label).toBe("Current project one");
        expect(registry.getSnapshot().recent.map((task) => task.label)).toEqual(["Three", "One updated"]);
    });

    it("bounds listeners and disposes idempotently with one immutable terminal snapshot", () => {
        const registry = new TaskRegistry({ maxListeners: 2 });
        const healthy = vi.fn();
        const unsubscribe = registry.subscribe(healthy);
        registry.subscribe(() => {
            throw new Error("listener failed");
        });
        expect(() => registry.subscribe(() => {})).toThrow(RangeError);
        registry.replaceSource("built-in", [definition("one")]);
        registry.replaceSource("built-in", [definition("two")]);
        expect(healthy).toHaveBeenCalledTimes(2);
        unsubscribe();
        unsubscribe();

        registry.dispose();
        const disposed = registry.getSnapshot();
        registry.dispose();
        expect(registry.getSnapshot()).toBe(disposed);
        expect(disposed).toMatchObject({ disposed: true, tasks: [], recent: [] });
        expect(() => registry.replaceSource("project", [])).toThrow(TaskRegistryDisposedError);
        expect(() => registry.rememberRecent(definition("later"))).toThrow(TaskRegistryDisposedError);
        expect(() => registry.subscribe(() => {})).toThrow(TaskRegistryDisposedError);
    });
});

describe("TaskController lifecycle", () => {
    it("runs a copied immutable task and returns to idle on completion", async () => {
        const running = runnerHandle();
        const runner: TaskRunner = { run: vi.fn(() => running.handle) };
        const controller = new TaskController(runner);
        const listener = vi.fn();
        controller.subscribe(listener);
        const input = resolvedTask("build", { env: { TOKEN: "original" } });

        const started = controller.run(input);
        (input.env as Record<string, string>).TOKEN = "mutated";
        expect(controller.getSnapshot()).toMatchObject({ status: "running", activeRunId: 1, runAttempts: 1, disposed: false });
        expect(Object.isFrozen(controller.getSnapshot())).toBe(true);
        expect(Object.isFrozen(controller.getSnapshot().failures)).toBe(true);
        await started;

        const launched = vi.mocked(runner.run).mock.calls[0]![0];
        expect(launched.env).toEqual({ TOKEN: "original" });
        expect(Object.isFrozen(launched)).toBe(true);
        expect(Object.isFrozen(launched.env)).toBe(true);
        await expect(controller.run(resolvedTask("other"))).rejects.toBeInstanceOf(TaskAlreadyRunningError);

        running.completion.resolve();
        await flushPromises();
        expect(controller.getSnapshot()).toMatchObject({ status: "idle", activeRunId: null, task: { id: "build" } });
        expect(listener).toHaveBeenCalledTimes(2);
    });

    it("stops exactly once when stop races asynchronous handle acquisition", async () => {
        const launch = deferred<TaskRunnerHandle>();
        const running = runnerHandle();
        const runner: TaskRunner = { run: vi.fn(() => launch.promise) };
        const controller = new TaskController(runner);

        const started = controller.run(resolvedTask("slow-start"));
        const firstStop = controller.stop();
        const secondStop = controller.stop();
        expect(secondStop).toBe(firstStop);
        expect(controller.getSnapshot().status).toBe("stopping");

        launch.resolve(running.handle);
        await started;
        await firstStop;
        expect(running.stop).toHaveBeenCalledOnce();
        expect(controller.getSnapshot()).toMatchObject({ status: "idle", activeRunId: null });

        running.completion.reject(new Error("stale completion"));
        await flushPromises();
        expect(controller.getSnapshot().failures).toEqual([]);
    });

    it("restarts through stop and ignores the old handle's stale completion", async () => {
        const stopGate = deferred<void>();
        const first = runnerHandle(() => stopGate.promise);
        const second = runnerHandle();
        const third = runnerHandle();
        const run = vi.fn().mockReturnValueOnce(first.handle).mockReturnValueOnce(second.handle).mockReturnValueOnce(third.handle);
        const controller = new TaskController({ run });
        await controller.run(resolvedTask("first"));

        const restart = controller.restart(resolvedTask("second"));
        expect(controller.restart(resolvedTask("ignored"))).toBe(restart);
        expect(controller.getSnapshot().status).toBe("stopping");
        first.completion.reject(new Error("old run failed late"));
        await flushPromises();
        expect(controller.getSnapshot().failures).toEqual([]);

        stopGate.resolve();
        await restart;
        expect(first.stop).toHaveBeenCalledOnce();
        expect(run).toHaveBeenCalledTimes(2);
        expect(controller.getSnapshot()).toMatchObject({ status: "running", activeRunId: 2, task: { id: "second" } });

        second.completion.resolve();
        await flushPromises();
        expect(controller.getSnapshot().status).toBe("idle");
        await controller.restart();
        expect(run).toHaveBeenCalledTimes(3);
    });

    it("records only sanitized bounded failure history without retaining opaque errors", async () => {
        const opaque = Object.create(null) as { message?: string; toString?: () => string };
        opaque.toString = () => {
            throw new Error("must not stringify");
        };
        const errors = [new Error("\u001b[31m first\nline"), "second\tmessage", opaque];
        const runner: TaskRunner = {
            run: vi.fn(() => {
                throw errors.shift();
            }),
        };
        let now = 40;
        const controller = new TaskController(runner, { maxFailureHistory: 2, now: () => now++ });

        await expect(controller.run(resolvedTask("one"))).rejects.toBeInstanceOf(Error);
        expect(controller.getSnapshot().failures[0]?.message).toBe("first line");
        await expect(controller.run(resolvedTask("two"))).rejects.toBe("second\tmessage");
        await expect(controller.run(resolvedTask("three"))).rejects.toBe(opaque);

        const snapshot = controller.getSnapshot();
        expect(snapshot.status).toBe("failed");
        expect(snapshot.failures).toHaveLength(2);
        expect(snapshot.failures.map((failure) => failure.sequence)).toEqual([2, 3]);
        expect(snapshot.failures.map((failure) => failure.at)).toEqual([41, 42]);
        expect(snapshot.failures.map((failure) => failure.message)).toEqual(["second message", "Task operation failed"]);
        expect(snapshot.failures.every((failure) => failure.operation === "run")).toBe(true);
        expect(snapshot.failures.every((failure) => failure.message.length <= TASK_REGISTRY_LIMITS.maxFailureMessageLength)).toBe(true);
        expect(Object.isFrozen(snapshot.failures[0])).toBe(true);
        expect(Object.keys(snapshot.failures[0]!)).not.toContain("error");
    });

    it("tracks completion failures and permits a fresh run", async () => {
        const first = runnerHandle();
        const second = runnerHandle();
        const run = vi.fn().mockReturnValueOnce(first.handle).mockReturnValueOnce(second.handle);
        const controller = new TaskController({ run }, { now: () => 7 });
        await controller.run(resolvedTask("test"));

        first.completion.reject(new Error("tests failed\nwith details"));
        await flushPromises();
        expect(controller.getSnapshot()).toMatchObject({ status: "failed", activeRunId: null });
        expect(controller.getSnapshot().failures).toEqual([
            {
                sequence: 1,
                at: 7,
                runId: 1,
                taskId: "test",
                project: "/workspace/project",
                operation: "completion",
                message: "tests failed with details",
            },
        ]);

        await controller.run(resolvedTask("retry"));
        expect(controller.getSnapshot()).toMatchObject({ status: "running", activeRunId: 2, task: { id: "retry" } });
    });

    it("records stop failures, deduplicates concurrent stops, and allows retry", async () => {
        let attempts = 0;
        const running = runnerHandle(() => {
            attempts += 1;
            if (attempts === 1) return Promise.reject(new Error("stop\nfailed"));
            return Promise.resolve();
        });
        const controller = new TaskController({ run: () => running.handle }, { now: () => 11 });
        await controller.run(resolvedTask("watch"));

        const first = controller.stop();
        expect(controller.stop()).toBe(first);
        await expect(first).rejects.toThrow("stop\nfailed");
        expect(running.stop).toHaveBeenCalledOnce();
        expect(controller.getSnapshot()).toMatchObject({ status: "failed", activeRunId: 1 });
        expect(controller.getSnapshot().failures[0]).toMatchObject({ operation: "stop", message: "stop failed" });

        await controller.stop();
        expect(running.stop).toHaveBeenCalledTimes(2);
        expect(controller.getSnapshot()).toMatchObject({ status: "idle", activeRunId: null });
    });

    it("disposes idempotently, stops ownership once, and rejects new work", async () => {
        const stopGate = deferred<void>();
        const running = runnerHandle(() => stopGate.promise);
        const controller = new TaskController({ run: () => running.handle });
        await controller.run(resolvedTask("serve"));

        const firstDispose = controller.dispose();
        const secondDispose = controller.dispose();
        expect(secondDispose).toBe(firstDispose);
        expect(controller.getSnapshot()).toMatchObject({ disposed: true, status: "stopping" });
        await expect(controller.run(resolvedTask("later"))).rejects.toBeInstanceOf(TaskControllerDisposedError);
        await expect(controller.restart()).rejects.toBeInstanceOf(TaskControllerDisposedError);

        stopGate.resolve();
        await firstDispose;
        expect(running.stop).toHaveBeenCalledOnce();
        expect(controller.getSnapshot()).toMatchObject({ disposed: true, status: "idle", activeRunId: null });
        expect(() => controller.subscribe(() => {})).toThrow(TaskControllerDisposedError);

        running.completion.reject(new Error("late completion"));
        await flushPromises();
        expect(controller.getSnapshot().failures).toEqual([]);
    });

    it("rejects restart without a prior task and invalid runner handles deterministically", async () => {
        const unavailable = new TaskController({ run: vi.fn() });
        await expect(unavailable.restart()).rejects.toBeInstanceOf(TaskUnavailableError);

        const invalid = new TaskController({
            run: () => ({ completion: Promise.resolve() }) as unknown as TaskRunnerHandle,
        });
        await expect(invalid.run(resolvedTask("invalid"))).rejects.toThrow(/handle/);
        expect(invalid.getSnapshot().status).toBe("failed");
        expect(invalid.getSnapshot().failures[0]).toMatchObject({ operation: "run" });
    });
});
