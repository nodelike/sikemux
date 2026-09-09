import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";

const initial = getState();

beforeEach(() => setState(initial, true));

function activeProjectTerminalIds(): string[] {
    const state = getState();
    return (state.windowsBySession[state.activeSessionId] ?? []).filter((id) => state.windows[id]?.role === "term");
}

describe("project terminal tabs", () => {
    it("creates the initial terminal as a regular numbered, closable tab", () => {
        cmd.createProjectSession("/work/demo");

        const [terminalId] = activeProjectTerminalIds();
        expect(getState().windows[terminalId]).toMatchObject({ name: "1", role: "term" });
        expect(getState().windows[terminalId].fixed).toBeUndefined();
    });

    it("numbers new terminals from the existing terminal tabs, not fixed project windows", () => {
        cmd.createProjectSession("/work/demo");
        cmd.newWindow();

        const labels = activeProjectTerminalIds().map((id) => getState().windows[id].name);
        expect(labels).toEqual(["1", "2"]);
    });

    it("can close the last project terminal instead of silently replacing it", () => {
        cmd.createProjectSession("/work/demo");
        const [terminalId] = activeProjectTerminalIds();
        cmd.selectWindowId(terminalId);

        cmd.closeActiveFocusTarget();

        expect(activeProjectTerminalIds()).toEqual([]);
        expect(getState().windows[terminalId]).toBeUndefined();
        // Editor, diff and search are opened on demand, so a project with no
        // terminal is left with no window tabs at all rather than a stale one.
        expect(getState().windowsBySession[getState().activeSessionId]).toEqual([]);
    });

    it("closes the initial terminal through the tab action and can reopen terminal 1", () => {
        cmd.createProjectSession("/work/demo");
        const [terminalId] = activeProjectTerminalIds();

        cmd.closeWindowById(terminalId);
        expect(activeProjectTerminalIds()).toEqual([]);

        cmd.selectWindowByRole("term");
        const [reopenedId] = activeProjectTerminalIds();
        expect(getState().windows[reopenedId]).toMatchObject({ name: "1", role: "term" });
        expect(getState().sessions[getState().activeSessionId].activeWindowId).toBe(reopenedId);
    });
});
