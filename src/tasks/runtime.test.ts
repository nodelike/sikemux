import { describe, expect, it, vi } from "vitest";
import { TaskAlreadyRunningError, TaskRegistry, type ResolvedTaskDefinition, type TaskDefinitionInput } from "./taskRegistry";
import {
    HeadlessPtyTaskRunner,
    TaskBackendCompletionError,
    TaskBackendProtocolError,
    TaskBackendStopError,
    TaskProcessExitError,
    TaskRuntime,
    TaskRuntimeCapacityError,
    TaskRuntimeDisposedError,
    TaskRuntimeTaskNotFoundError,
    TaskTerminalSurfaceError,
    type TaskExecutionBackend,
    type TaskExecutionRequest,
    type TaskProcessExit,
    type TaskTerminalOpenRequest,
} from "./runtime";

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

function definition(id: string, project = "/workspace/project", overrides: Partial<TaskDefinitionInput> = {}): TaskDefinitionInput {
    return {
        id,
        label: `Task ${id}`,
        project,
        command: `run ${id}`,
        cwd: project,
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

async function flushPromises(): Promise<void> {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
}

interface BackendRun {
    readonly ptyId: number;
    readonly exit: Deferred<TaskProcessExit>;
    readonly request: TaskExecutionRequest;
}

function runtimeHarness(definitions: readonly TaskDefinitionInput[], maxProjectControllers = 4) {
    const registry = new TaskRegistry();
    registry.replaceSource("project", definitions);
    const runs: BackendRun[] = [];
    let nextPtyId = 100;
    const stop = vi.fn(async (_ptyId: number) => undefined);
    const backend: TaskExecutionBackend = {
        start: vi.fn((request) => {
            const run = { ptyId: nextPtyId, exit: deferred<TaskProcessExit>(), request };
            nextPtyId += 1;
            runs.push(run);
            return { ptyId: run.ptyId, completion: run.exit.promise };
        }),
        stop,
    };
    const open = vi.fn(async (_request: TaskTerminalOpenRequest) => undefined);
    const runtime = new TaskRuntime({ registry, backend, surface: { open }, maxProjectControllers });
    return { registry, runs, stop, open, runtime };
}

describe("HeadlessPtyTaskRunner", () => {
    it("preserves structured launch data and surfaces only the exact existing PTY", async () => {
        const exit = deferred<TaskProcessExit>();
        const start = vi.fn((_request: TaskExecutionRequest) => ({ ptyId: 42, completion: exit.promise }));
        const stop = vi.fn();
        const open = vi.fn();
        const runner = new HeadlessPtyTaskRunner({ backend: { start, stop }, surface: { open }, cols: 132, rows: 43 });
        const task = resolvedTask("build", {
            label: "Build app",
            command: "printf '$TOKEN' && pnpm build",
            cwd: "/workspace/project with spaces",
            env: { TOKEN: "secret", Z_LAST: "z" },
        });

        const handle = await runner.run(task);
        const request = start.mock.calls[0]![0];
        expect(request).toMatchObject({
            taskId: "build",
            label: "Build app",
            project: "/workspace/project",
            source: "project",
            command: "printf '$TOKEN' && pnpm build",
            cwd: "/workspace/project with spaces",
            env: { TOKEN: "secret", Z_LAST: "z" },
            cols: 132,
            rows: 43,
        });
        expect(Object.isFrozen(request)).toBe(true);
        expect(Object.isFrozen(request.env)).toBe(true);
        expect(request.env).not.toBe(task.env);

        const surfaceRequest = open.mock.calls[0]![0];
        expect(surfaceRequest).toMatchObject({
            executionId: request.executionId,
            terminalKey: request.terminalKey,
            ptyId: 42,
            taskId: "build",
            project: "/workspace/project",
            cwd: "/workspace/project with spaces",
        });
        expect(Object.isFrozen(surfaceRequest)).toBe(true);
        expect("command" in surfaceRequest).toBe(false);
        expect("env" in surfaceRequest).toBe(false);
        expect(surfaceRequest.signal.aborted).toBe(false);

        exit.resolve({ code: 0 });
        await expect(handle.completion).resolves.toBeUndefined();
        // A fast task must still be allowed to finish opening its retained
        // output terminal after the process has exited.
        expect(surfaceRequest.signal.aborted).toBe(false);
        expect(stop).not.toHaveBeenCalled();
    });

    it("stops the exact owned PTY once and preserves promise identity", async () => {
        const exit = deferred<TaskProcessExit>();
        const stopGate = deferred<void>();
        const stop = vi.fn((_ptyId: number) => stopGate.promise);
        const surfaceRequests: TaskTerminalOpenRequest[] = [];
        const runner = new HeadlessPtyTaskRunner({
            backend: { start: () => ({ ptyId: 73, completion: exit.promise }), stop },
            surface: { open: (request) => void surfaceRequests.push(request) },
        });
        const handle = await runner.run(resolvedTask("watch"));

        const first = handle.stop() as Promise<void>;
        const second = handle.stop() as Promise<void>;
        expect(second).toBe(first);
        expect(stop).toHaveBeenCalledOnce();
        expect(stop).toHaveBeenCalledWith(73);
        expect(surfaceRequests[0]!.signal.aborted).toBe(true);

        stopGate.resolve();
        await first;
        await expect(handle.completion).resolves.toBeUndefined();
        expect(stop).toHaveBeenCalledOnce();
    });

    it("waits for terminal presentation when a fast task exits first", async () => {
        const opened = deferred<void>();
        const runner = new HeadlessPtyTaskRunner({
            backend: { start: () => ({ ptyId: 74, completion: Promise.resolve({ code: 0 }) }), stop: vi.fn() },
            surface: { open: () => opened.promise },
        });
        const handle = await runner.run(resolvedTask("fast"));
        const settled = vi.fn();
        void Promise.resolve(handle.completion).then(settled);
        await flushPromises();
        expect(settled).not.toHaveBeenCalled();

        opened.resolve();
        await expect(handle.completion).resolves.toBeUndefined();
        expect(settled).toHaveBeenCalledOnce();
    });

    it("reserves presentation failure, stops its PTY, and never exposes the opaque error", async () => {
        const exit = deferred<TaskProcessExit>();
        const stop = vi.fn(async (_ptyId: number) => undefined);
        const runner = new HeadlessPtyTaskRunner({
            backend: { start: () => ({ ptyId: 81, completion: exit.promise }), stop },
            surface: { open: () => Promise.reject(new Error("secret renderer internals")) },
        });
        const handle = await runner.run(resolvedTask("present"));
        exit.resolve({ code: 0 });

        const failure = await Promise.resolve(handle.completion).catch((error: unknown) => error);
        expect(failure).toBeInstanceOf(TaskTerminalSurfaceError);
        expect((failure as Error).message).not.toContain("secret renderer internals");
        expect(stop).toHaveBeenCalledOnce();
        expect(stop).toHaveBeenCalledWith(81);
    });

    it("maps process and stop failures to bounded typed errors", async () => {
        const failedExit = deferred<TaskProcessExit>();
        const first = new HeadlessPtyTaskRunner({
            backend: { start: () => ({ ptyId: 91, completion: failedExit.promise }), stop: vi.fn() },
            surface: { open: vi.fn() },
        });
        const failedHandle = await first.run(resolvedTask("fail"));
        failedExit.resolve({ code: 17, signal: "SIGTERM" });
        await expect(failedHandle.completion).rejects.toEqual(expect.objectContaining({ exitCode: 17, signal: "SIGTERM" }));
        await expect(failedHandle.completion).rejects.toBeInstanceOf(TaskProcessExitError);

        const opaqueExit = deferred<TaskProcessExit>();
        const second = new HeadlessPtyTaskRunner({
            backend: {
                start: () => ({ ptyId: 92, completion: opaqueExit.promise }),
                stop: () => Promise.reject(new Error("secret native stop")),
            },
            surface: { open: vi.fn() },
        });
        const stoppedHandle = await second.run(resolvedTask("stop-fail"));
        await expect(stoppedHandle.stop()).rejects.toBeInstanceOf(TaskBackendStopError);
        await expect(stoppedHandle.completion).rejects.toBeInstanceOf(TaskBackendStopError);
        await expect(stoppedHandle.stop()).rejects.not.toThrow("secret native stop");

        const rejectedExit = deferred<TaskProcessExit>();
        const rejectedExitStop = vi.fn();
        const third = new HeadlessPtyTaskRunner({
            backend: { start: () => ({ ptyId: 93, completion: rejectedExit.promise }), stop: rejectedExitStop },
            surface: { open: vi.fn() },
        });
        const rejectedHandle = await third.run(resolvedTask("status-fail"));
        rejectedExit.reject(new Error("secret status transport"));
        await expect(rejectedHandle.completion).rejects.toBeInstanceOf(TaskBackendCompletionError);
        await expect(rejectedHandle.completion).rejects.not.toThrow("secret status transport");
        expect(rejectedExitStop).toHaveBeenCalledOnce();
        expect(rejectedExitStop).toHaveBeenCalledWith(93);

        const malformedExit = deferred<TaskProcessExit>();
        const malformedExitStop = vi.fn();
        const fourth = new HeadlessPtyTaskRunner({
            backend: { start: () => ({ ptyId: 94, completion: malformedExit.promise }), stop: malformedExitStop },
            surface: { open: vi.fn() },
        });
        const malformedHandle = await fourth.run(resolvedTask("malformed-status"));
        malformedExit.resolve({ code: Number.NaN });
        await expect(malformedHandle.completion).rejects.toBeInstanceOf(TaskBackendProtocolError);
        expect(malformedExitStop).toHaveBeenCalledWith(94);
    });

    it("cleans an exact transferred PTY when the backend result is malformed", async () => {
        const stop = vi.fn(async (_ptyId: number) => undefined);
        const runner = new HeadlessPtyTaskRunner({
            backend: {
                start: () => ({ ptyId: 117, completion: null }) as unknown as ReturnType<TaskExecutionBackend["start"]>,
                stop,
            },
            surface: { open: vi.fn() },
        });

        await expect(runner.run(resolvedTask("invalid"))).rejects.toBeInstanceOf(TaskBackendProtocolError);
        expect(stop).toHaveBeenCalledOnce();
        expect(stop).toHaveBeenCalledWith(117);
    });
});

describe("TaskRuntime", () => {
    it("shows a task the core kept as running, opens its terminal quietly, and stops that exact process", async () => {
        const { runtime, open, stop } = runtimeHarness([definition("dev")]);
        const exit = deferred<TaskProcessExit>();
        await runtime.adopt(resolvedTask("dev", { env: {} }), "kept-execution", { ptyId: 555, completion: exit.promise });
        await flushPromises();

        expect(runtime.getSnapshot("/workspace/project")).toMatchObject({ status: "running", task: { id: "dev" } });
        expect(open).toHaveBeenCalledWith(expect.objectContaining({ ptyId: 555, executionId: "kept-execution", background: true }));

        await runtime.stop("/workspace/project");
        expect(stop).toHaveBeenCalledWith(555);
        expect(runtime.getSnapshot("/workspace/project")?.status).toBe("idle");
    });

    it("lets a kept task finish on its own", async () => {
        const { runtime } = runtimeHarness([definition("dev")]);
        const exit = deferred<TaskProcessExit>();
        await runtime.adopt(resolvedTask("dev"), "kept-execution", { ptyId: 9, completion: exit.promise });
        exit.resolve({ code: 0 });
        await flushPromises();
        expect(runtime.getSnapshot("/workspace/project")?.status).toBe("idle");
    });

    it("runs one task per project, records recency after start, and observes completion", async () => {
        const harness = runtimeHarness([definition("build"), definition("test")]);

        await harness.runtime.run("/workspace/project", "build");
        expect(harness.runtime.getSnapshot("/workspace/project")).toMatchObject({ status: "running", activeRunId: 1, runAttempts: 1 });
        expect(harness.registry.getSnapshot().recent.map((task) => task.id)).toEqual(["build"]);
        await expect(harness.runtime.run("/workspace/project", "test")).rejects.toBeInstanceOf(TaskAlreadyRunningError);
        await expect(harness.runtime.run("/workspace/project", "missing")).rejects.toBeInstanceOf(TaskRuntimeTaskNotFoundError);

        harness.runs[0]!.exit.resolve({ code: 0 });
        await flushPromises();
        expect(harness.runtime.getSnapshot("/workspace/project")).toMatchObject({ status: "idle", activeRunId: null, task: { id: "build" } });
        await harness.runtime.dispose();
    });

    it("bounds project controllers and evicts only an inactive least-recently-used controller", async () => {
        const firstProject = "/workspace/one";
        const secondProject = "/workspace/two";
        const harness = runtimeHarness([definition("build", firstProject), definition("build", secondProject)], 1);
        await harness.runtime.run(firstProject, "build");

        await expect(harness.runtime.run(secondProject, "build")).rejects.toBeInstanceOf(TaskRuntimeCapacityError);
        expect(harness.runs).toHaveLength(1);
        harness.runs[0]!.exit.resolve({ code: 0 });
        await flushPromises();

        await harness.runtime.run(secondProject, "build");
        expect(harness.runs).toHaveLength(2);
        expect(harness.runtime.getSnapshot(firstProject)).toBeNull();
        expect(harness.runtime.getSnapshot(secondProject)?.status).toBe("running");
        await harness.runtime.dispose();
        expect(harness.stop).toHaveBeenLastCalledWith(101);
    });

    it("disposes all exact active PTYs once and rejects later project operations", async () => {
        const harness = runtimeHarness([definition("one", "/workspace/one"), definition("two", "/workspace/two")]);
        await harness.runtime.run("/workspace/one", "one");
        await harness.runtime.run("/workspace/two", "two");

        const first = harness.runtime.dispose();
        const second = harness.runtime.dispose();
        expect(second).toBe(first);
        await first;
        expect(harness.stop.mock.calls.map(([ptyId]) => ptyId).sort()).toEqual([100, 101]);
        await expect(harness.runtime.run("/workspace/one", "one")).rejects.toBeInstanceOf(TaskRuntimeDisposedError);
        await expect(harness.runtime.stop("/workspace/one")).rejects.toBeInstanceOf(TaskRuntimeDisposedError);
    });
});
