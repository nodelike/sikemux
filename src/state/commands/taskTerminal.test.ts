import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";
import type { TaskTerminalPresentationRequest } from "../../tasks/nativeRuntime";
import { taskPtyBindings } from "../../tasks/nativeRuntime";

const initial = getState();

beforeEach(() => {
    taskPtyBindings.reset();
    setState(initial, true);
});

function presentation(overrides: Partial<TaskTerminalPresentationRequest> = {}): TaskTerminalPresentationRequest {
    return {
        executionId: "execution-1",
        terminalKey: "task:test:/work/demo",
        taskId: "test",
        label: "Test",
        project: "/work/demo",
        source: "project",
        cwd: "/work/demo",
        signal: new AbortController().signal,
        ...overrides,
    };
}

describe("task terminal presentation", () => {
    it("creates a transient secret-free window in the owning project", () => {
        cmd.createProjectSession("/work/demo");
        const paneId = cmd.openTaskTerminal(presentation());
        const state = getState();
        const session = state.sessions[state.activeSessionId];
        const window = state.windows[session.activeWindowId];

        expect(window).toMatchObject({ name: "Test", role: "term", transient: true, activePaneId: paneId });
        expect(window.root).toMatchObject({
            type: "pane",
            id: paneId,
            cwd: "/work/demo",
            title: "Test",
            externalPty: true,
            taskTerminalKey: "task:test:/work/demo",
        });
        expect(JSON.stringify(window)).not.toContain("pnpm test");
        expect(JSON.stringify(window)).not.toContain("NODE_ENV");
    });

    it("reuses the same task pane while updating safe presentation metadata", () => {
        cmd.createProjectSession("/work/demo");
        const first = cmd.openTaskTerminal(presentation());
        const second = cmd.openTaskTerminal(presentation({ executionId: "execution-2", label: "Test again", cwd: "/work/demo/package" }));
        const state = getState();
        const taskWindows = Object.values(state.windows).filter((window) => window.transient);

        expect(second).toBe(first);
        expect(taskWindows).toHaveLength(1);
        expect(taskWindows[0]).toMatchObject({ name: "Test again", root: { id: first, cwd: "/work/demo/package", title: "Test again" } });
    });

    it("rejects closed projects, escaped directories, and cancelled presentation", () => {
        expect(() => cmd.openTaskTerminal(presentation())).toThrow("no longer open");
        cmd.createProjectSession("/work/demo");
        expect(() => cmd.openTaskTerminal(presentation({ cwd: "/work/outside" }))).toThrow("stay within");
        const abort = new AbortController();
        abort.abort(new Error("cancelled"));
        expect(() => cmd.openTaskTerminal(presentation({ signal: abort.signal }))).toThrow("cancelled");
    });

    it("releases runtime bindings when the task pane or project closes", () => {
        cmd.createProjectSession("/work/demo");
        const sessionId = getState().activeSessionId;
        const firstPane = cmd.openTaskTerminal(presentation());
        taskPtyBindings.bind(firstPane, { ptyId: 7, executionId: "one", terminalKey: "task:test:/work/demo" });

        cmd.closeActiveFocusTarget();
        expect(taskPtyBindings.getSnapshot(firstPane)).toBeNull();

        const secondPane = cmd.openTaskTerminal(presentation({ executionId: "two" }));
        taskPtyBindings.bind(secondPane, { ptyId: 8, executionId: "two", terminalKey: "task:test:/work/demo" });
        cmd.closeSession(sessionId);
        expect(taskPtyBindings.getSnapshot(secondPane)).toBeNull();
    });
});
