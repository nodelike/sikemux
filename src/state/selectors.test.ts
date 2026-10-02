import { describe, expect, it } from "vitest";
import { getState, setState } from "./store";
import { selectActiveSession, selectSession, selectSessionIds, selectWindowIds } from "./selectors";

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

    it("derives the active session without projecting whole maps", () => {
        const state = getState();
        expect(selectActiveSession(state)?.id).toBe(state.activeSessionId);
    });
});
