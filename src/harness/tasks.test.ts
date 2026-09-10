import { describe, expect, it, vi } from "vitest";
import { HarnessTasks } from "./tasks";
import { HarnessEvents } from "./events";
import type { TaskProcessExit, TaskExecutionRequest } from "../tasks/runtime";

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((done) => {
        resolve = done;
    });
    return { promise, resolve };
}
function fixture() {
    const exits: ReturnType<typeof deferred<TaskProcessExit>>[] = [];
    const backend = {
        start: vi.fn(() => {
            const completion = deferred<TaskProcessExit>();
            exits.push(completion);
            return { ptyId: exits.length, completion: completion.promise };
        }),
        stop: vi.fn(async () => {}),
    };
    const surface = { open: vi.fn(async () => {}) };
    const events = new HarnessEvents();
    return { tasks: new HarnessTasks(backend, surface, events), backend, surface, events, exits };
}
const request: Omit<TaskExecutionRequest, "executionId" | "terminalKey"> = {
    taskId: "dev",
    label: "Dev",
    project: "/one",
    source: "project",
    command: "echo test",
    cwd: "/one",
    env: {},
    cols: 80,
    rows: 24,
};

describe("managed harness tasks", () => {
    it("deduplicates concurrent launches and retries, including aliases after completion", async () => {
        const { tasks, backend, exits } = fixture();
        const [first, retry, alias] = await Promise.all([tasks.start(request, "key"), tasks.start(request, "key"), tasks.start(request, "alias")]);
        expect(backend.start).toHaveBeenCalledOnce();
        expect(retry.executionId).toBe(first.executionId);
        expect(alias.executionId).toBe(first.executionId);
        exits[0].resolve({ code: 0 });
        await Promise.resolve();
        expect((await tasks.start(request, "alias")).status).toBe("completed");
        expect(() => tasks.start({ ...request, taskId: "other" }, "key")).toThrow("another task");
    });
    it("stops only the requested generation and keeps independent tasks running", async () => {
        const { tasks, backend, exits } = fixture();
        const first = await tasks.start(request, "one");
        exits[0].resolve({ code: 0 });
        await Promise.resolve();
        const second = await tasks.start(request, "two");
        await tasks.stop("/one", first.executionId);
        expect(backend.stop).not.toHaveBeenCalled();
        await Promise.all([tasks.stop("/one", second.executionId), tasks.stop("/one", second.executionId)]);
        expect(backend.stop).toHaveBeenCalledExactlyOnceWith(2);
        expect(tasks.get("/one", second.executionId).status).toBe("stopped");
        expect(() => tasks.get("/two", second.executionId)).toThrow("does not belong");
    });
    it("preserves failure exit codes and produces output/lifecycle events", async () => {
        const { tasks, events, exits } = fixture();
        const cursor = events.cursor;
        const run = await tasks.start(request, "key");
        tasks.output(run.ptyId!);
        exits[0].resolve({ code: 7 });
        await Promise.resolve();
        expect(tasks.get("/one", run.executionId)).toMatchObject({ status: "failed", exitCode: 7 });
        expect((await events.wait("/one", cursor, 0)).events.map((event) => event.kind)).toEqual([
            "task.starting",
            "task.running",
            "task.output",
            "task.failed",
        ]);
    });
    it("cleans up failed presentation and allows retrying a failed stop", async () => {
        const { tasks, backend, surface } = fixture();
        surface.open.mockRejectedValueOnce(new Error("pane closed"));
        await expect(tasks.start(request, "bad")).rejects.toThrow("pane closed");
        expect(backend.stop).toHaveBeenCalledWith(1);
        const run = await tasks.start(request, "good");
        backend.stop.mockRejectedValueOnce(new Error("stop failed"));
        await expect(tasks.stop("/one", run.executionId)).rejects.toThrow("stop failed");
        expect((await tasks.stop("/one", run.executionId)).status).toBe("stopped");
    });
    it("waits for pending launch before stopping its exact PTY", async () => {
        const started = deferred<{ ptyId: number; completion: Promise<TaskProcessExit> }>();
        const backend = { start: vi.fn(() => started.promise), stop: vi.fn(async () => {}) };
        const tasks = new HarnessTasks(backend, { open: async () => {} }, new HarnessEvents());
        const launch = tasks.start(request, "key");
        const run = tasks.list("/one")[0];
        const stop = tasks.stop("/one", run.executionId);
        expect(backend.stop).not.toHaveBeenCalled();
        started.resolve({ ptyId: 71, completion: new Promise(() => {}) });
        await launch;
        await stop;
        expect(backend.stop).toHaveBeenCalledExactlyOnceWith(71);
    });
});
