import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { activeTaskControls, gitChangeInvalidates, subscribeGitChanged } from "./App";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "./api/transport";
import { subscribe } from "./state/bus";
import type { ResolvedTaskDefinition, TaskControllerSnapshot, TaskLifecycleState } from "./tasks/taskRegistry";

let transport: MemoryIpcTransport;

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
});

afterEach(() => resetIpcTransportForTests());

function task(id: string): ResolvedTaskDefinition {
    return Object.freeze({
        id,
        label: id,
        project: "/repo",
        command: `run ${id}`,
        cwd: "/repo",
        env: Object.freeze({}),
        source: "project",
    });
}

function snapshot(status: TaskLifecycleState, activeRunId: number | null, activeTask: ResolvedTaskDefinition | null): TaskControllerSnapshot {
    return Object.freeze({
        revision: 1,
        status,
        disposed: false,
        task: activeTask,
        activeRunId,
        runAttempts: 1,
        failures: Object.freeze([]),
    });
}

describe("active task command reachability", () => {
    it.each(["running", "stopping", "failed"] as const)("keeps Stop reachable for an owned PTY in %s state after discovery clears", (status) => {
        expect(activeTaskControls(snapshot(status, 7, task("watch")), [])).toEqual({
            canStop: true,
            restartTaskId: null,
        });
    });

    it("allows restart only through the current project inventory", () => {
        const running = snapshot("running", 7, task("watch"));

        expect(activeTaskControls(running, [task("watch")])).toEqual({ canStop: true, restartTaskId: "watch" });
        expect(activeTaskControls(running, [task("build")])).toEqual({ canStop: true, restartTaskId: null });
    });

    it("does not expose Stop for a settled or disposed controller", () => {
        expect(activeTaskControls(snapshot("failed", null, task("watch")), [task("watch")])).toEqual({
            canStop: false,
            restartTaskId: "watch",
        });
        expect(activeTaskControls({ ...snapshot("running", 7, task("watch")), disposed: true }, [task("watch")])).toEqual({
            canStop: false,
            restartTaskId: null,
        });
    });
});

describe("Git change IPC subscription", () => {
    it("keeps file changes to status resources and reserves broad Git refreshes for metadata signals", () => {
        expect(gitChangeInvalidates("files.list", ["/repo"], "/repo", ["src/file.ts"])).toBe(true);
        expect(gitChangeInvalidates("git.overview", ["/repo"], "/repo", ["src/file.ts"])).toBe(true);
        expect(gitChangeInvalidates("git.status", ["/repo"], "/repo", ["src/file.ts"])).toBe(true);
        expect(gitChangeInvalidates("git.stashes", ["/repo"], "/repo", ["src/file.ts"])).toBe(false);
        expect(gitChangeInvalidates("git.remotes", ["/repo"], "/repo", null)).toBe(true);
        expect(gitChangeInvalidates("git.status", ["/other"], "/repo", null)).toBe(false);
    });

    it("routes through the installed transport and aborts idempotently", async () => {
        const changed = vi.fn();
        const unsubscribeBus = subscribe("fs-changed", changed);
        const controller = new AbortController();
        const unsubscribe = await subscribeGitChanged(controller.signal);

        expect(transport.eventListenerCount).toBe(1);
        transport.emit("git_changed", { repo: "/repo", paths: null });
        expect(changed).toHaveBeenCalledWith({ type: "fs-changed", repo: "/repo" });
        transport.emit("git_changed", { repo: "/repo", paths: ["src/file.ts"] });
        expect(changed).toHaveBeenLastCalledWith({ type: "fs-changed", repo: "/repo", paths: ["src/file.ts"] });

        controller.abort();
        controller.abort();
        unsubscribe();
        expect(transport.eventListenerCount).toBe(0);
        unsubscribeBus();
    });
});
