import { beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi } from "../../api/browser";
import * as cmd from "../commands";
import { acceptDialog, resetDialogsForTests, useDialogs } from "../dialog";
import { BROWSER_ACTIVE } from "../desks";
import { collectPanes } from "../layout";
import { agentIdsOf } from "../selectors";
import { getState, setState } from "../store";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    resetDialogsForTests();
});

function startAgent(): string {
    cmd.addAgent("claude");
    const st = getState();
    return agentIdsOf(st, st.activeSessionId).at(-1)!;
}

describe("closing what holds agents", () => {
    it("closes an idle agent straight away", () => {
        cmd.createProjectSession("/work/demo");
        const agentId = startAgent();

        cmd.closeActiveFocusTarget();

        expect(getState().agents[agentId]).toBeUndefined();
        expect(useDialogs.getState().dialog).toBeNull();
    });

    it("asks before stopping an agent that is still working", async () => {
        cmd.createProjectSession("/work/demo");
        const agentId = startAgent();
        cmd.noteAgentActivity(agentId, "working");

        cmd.closeActiveFocusTarget();

        const dialog = useDialogs.getState().dialog;
        expect(dialog?.title).toBe("Close claude?");
        expect(getState().agents[agentId]).toBeDefined();
        acceptDialog(dialog!.id);
        await Promise.resolve();
        expect(getState().agents[agentId]).toBeUndefined();
    });

    it("asks before closing a project that has agents in it", () => {
        cmd.createProjectSession("/work/one");
        cmd.createProjectSession("/work/two");
        const agentId = startAgent();
        const sessionId = getState().activeSessionId;

        cmd.closeActiveSession();

        expect(useDialogs.getState().dialog?.title).toBe("Close two?");
        expect(getState().sessions[sessionId]).toBeDefined();
        expect(getState().agents[agentId]).toBeDefined();
    });
});

describe("closing with the desk in front", () => {
    function deskKinds(): string[] {
        const st = getState();
        const win = st.windows[st.sessions[st.activeSessionId].activeWindowId];
        return collectPanes(win.root).map((pane) => pane.kind);
    }

    it("closes the page it shows rather than hiding the desk with the page still on it", () => {
        const close = vi.spyOn(browserApi, "closeTab").mockResolvedValue(undefined);
        cmd.createProjectSession("/work/demo");
        const agentId = startAgent();
        cmd.openDesk(agentId);
        const tab = {
            id: "tab-1",
            title: "Docs",
            url: "https://example.com",
            active: true,
            loading: false,
            canGoBack: false,
            canGoForward: false,
            favicon: null,
            acting: false,
        };
        setState((st) => ({
            browserStrips: { ...st.browserStrips, [agentId]: { tabs: [tab], activeTabId: "tab-1" } },
            desks: { ...st.desks, [agentId]: { ...st.desks[agentId], active: BROWSER_ACTIVE } },
        }));

        cmd.closeActiveFocusTarget();

        expect(close).toHaveBeenCalledWith(agentId, "tab-1");
        expect(deskKinds()).toEqual(["agent", "desk"]);
        expect(getState().agents[agentId]).toBeDefined();
        close.mockRestore();
    });

    it("hides a desk with nothing on it", () => {
        cmd.createProjectSession("/work/demo");
        const agentId = startAgent();
        cmd.openDesk(agentId);

        cmd.closeActiveFocusTarget();

        expect(deskKinds()).toEqual(["agent"]);
        expect(getState().agents[agentId]).toBeDefined();
    });
});
