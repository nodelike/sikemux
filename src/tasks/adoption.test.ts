import { describe, expect, it, vi } from "vitest";
import type { CoreSession, CoreTaskInfo } from "../api/coreSessions";
import { adoptCoreTasks, taskOrigin, type TaskAdoptionTargets } from "./adoption";

function taskInfo(kind: string, taskId: string, overrides: Partial<CoreTaskInfo> = {}): CoreTaskInfo {
    return {
        executionId: `exec-${taskId}`,
        terminalKey: JSON.stringify([kind, "/repo", taskId]),
        taskId,
        label: taskId,
        project: "/repo",
        source: "project",
        command: `run ${taskId}`,
        cwd: "/repo",
        agentId: null,
        ...overrides,
    };
}

function taskSession(id: number, task: CoreTaskInfo, running = true): CoreSession {
    return {
        id,
        kind: "task",
        pid: 1,
        running,
        project: "/repo",
        paneId: null,
        agentId: null,
        agentType: null,
        task,
        exit: null,
        killed: false,
        startedBy: null,
    };
}

function targets() {
    return {
        watch: vi.fn(async (ptyId: number) => ({ ptyId, completion: new Promise<never>(() => {}) })),
        adoptDeckTask: vi.fn(async (..._args: Parameters<TaskAdoptionTargets["adoptDeckTask"]>) => {}),
        showHarnessTerminal: vi.fn((..._args: Parameters<TaskAdoptionTargets["showHarnessTerminal"]>) => {}),
    } satisfies TaskAdoptionTargets;
}

describe("taking back tasks the core kept", () => {
    it("reads where a task came from out of its terminal key", () => {
        expect(taskOrigin(taskInfo("task", "dev"))).toEqual({ kind: "deck", project: "/repo", taskId: "dev" });
        expect(taskOrigin(taskInfo("harness", "sh:build-1a2b3c"))).toEqual({ kind: "harness", project: "/repo", taskId: "sh:build-1a2b3c" });
        expect(taskOrigin(taskInfo("task", "dev", { terminalKey: "not json" }))).toBeNull();
        expect(taskOrigin(taskInfo("task", "dev", { terminalKey: JSON.stringify(["task", "/elsewhere", "dev"]) }))).toBeNull();
    });

    it("returns running deck tasks to the deck and reopens running harness terminals without taking their runs", async () => {
        const target = targets();
        const running = await adoptCoreTasks(
            [
                taskSession(1, taskInfo("task", "dev")),
                taskSession(2, taskInfo("task", "lint"), false),
                taskSession(3, taskInfo("harness", "test", { agentId: "agent-1" })),
                taskSession(4, taskInfo("harness", "build"), false),
                { ...taskSession(5, taskInfo("task", "x")), kind: "terminal" },
            ],
            target,
        );
        expect(running).toBe(2);
        expect(target.watch.mock.calls.map(([id]) => id)).toEqual([1]);
        expect(target.adoptDeckTask).toHaveBeenCalledWith(
            expect.objectContaining({ id: "dev", project: "/repo", command: "run dev", env: {} }),
            "exec-dev",
            expect.objectContaining({ ptyId: 1 }),
        );
        expect(target.showHarnessTerminal).toHaveBeenCalledTimes(1);
        expect(target.showHarnessTerminal).toHaveBeenCalledWith(
            expect.objectContaining({ executionId: "exec-test", ptyId: 3, taskId: "test", agentId: "agent-1" }),
        );
    });

    it("skips a task whose exit cannot be watched", async () => {
        const target = targets();
        target.watch.mockRejectedValue(new Error("gone"));
        expect(await adoptCoreTasks([taskSession(1, taskInfo("task", "dev"))], target)).toBe(0);
        expect(target.adoptDeckTask).not.toHaveBeenCalled();
    });
});
