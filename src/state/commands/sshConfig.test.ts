import { beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";

vi.mock("../../api/ssh", () => ({
    sshApi: {
        configEnsure: vi.fn().mockResolvedValue("/Users/test/.ssh/config"),
    },
}));

const initial = getState();

beforeEach(() => {
    setState(initial, true);
});

describe("SSH config window", () => {
    it("opens config in its own singleton SSH session", async () => {
        const projectSessionId = getState().activeSessionId;
        const projectWindows = getState().windowsBySession[projectSessionId];
        const before = getState().sessionOrder.length;

        await cmd.openSshConfigEditor();
        let state = getState();
        const sessionId = state.activeSessionId;
        const session = state.sessions[sessionId];
        const windowId = session.activeWindowId;
        const root = state.windows[windowId].root;
        expect(sessionId).not.toBe(projectSessionId);
        expect(session).toMatchObject({ kind: "ssh", name: "SSH config", cwd: "/Users/test/.ssh" });
        expect(state.sessionOrder).toHaveLength(before + 1);
        expect(state.windowsBySession[projectSessionId]).toEqual(projectWindows);
        expect(state.windowsBySession[sessionId]).toEqual([windowId]);
        expect(state.windows[windowId].role).toBe("ssh-config");
        expect(root).toMatchObject({ type: "pane", kind: "editor", cwd: "/Users/test/.ssh" });
        expect(root.type).toBe("pane");
        if (root.type !== "pane") throw new Error("expected editor pane");
        expect(state.editorViews[root.id]).toEqual({
            openTabs: ["/Users/test/.ssh/config"],
            activePath: "/Users/test/.ssh/config",
        });

        await cmd.openSshConfigEditor();
        expect(getState().sessionOrder).toHaveLength(before + 1);
        expect(getState().activeSessionId).toBe(sessionId);

        cmd.closeActiveFocusTarget();
        state = getState();
        expect(state.sessions[sessionId]).toBeUndefined();
        expect(state.windows[windowId]).toBeUndefined();
        expect(state.sessionOrder).toHaveLength(before);
        expect(state.activeSessionId).toBe(projectSessionId);
    });
});
