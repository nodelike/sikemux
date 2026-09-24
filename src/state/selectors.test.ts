import { describe, expect, it } from "vitest";
import { getState, setState } from "./store";
import { selectActiveSession, selectActiveWindow, selectItemState, selectSession, selectSessionIds, selectWindowIds } from "./selectors";

describe("narrow store selectors", () => {
    it("returns stable entity and index references across unrelated updates", () => {
        const before = getState();
        const sessionId = before.activeSessionId;
        const session = selectSession(sessionId)(before);
        const sessionIds = selectSessionIds(before);
        const windowIds = selectWindowIds(sessionId)(before);

        setState({ commandPaletteOpen: !before.commandPaletteOpen });
        const after = getState();

        expect(selectSession(sessionId)(after)).toBe(session);
        expect(selectSessionIds(after)).toBe(sessionIds);
        expect(selectWindowIds(sessionId)(after)).toBe(windowIds);
    });

    it("derives the active session and window without projecting whole maps", () => {
        const state = getState();
        expect(selectActiveSession(state)?.id).toBe(state.activeSessionId);
        expect(selectActiveWindow(state)?.id).toBe(state.sessions[state.activeSessionId]?.activeWindowId);
    });

    it("adapts existing item-local state maps by kind", () => {
        const state = getState();
        expect(selectItemState(state, "terminal", "pane")).toBeUndefined();
        expect(selectItemState(state, "agent", "pane")).toBeUndefined();
        expect(selectItemState(state, "editor", "missing")).toBeUndefined();
    });
});
