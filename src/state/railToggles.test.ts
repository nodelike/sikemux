import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { toggleTabRail, toggleSideRail, toggleZen } from "./commands";
import { selectFocusMode } from "./selectors";
import { getState, setState } from "./store";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
});

describe("rail toggles", () => {
    it("toggles each rail on its own", () => {
        setState({ sideRailOpen: true, tabRailOpen: true });

        toggleSideRail();
        expect(getState()).toMatchObject({ sideRailOpen: false, tabRailOpen: true });

        toggleSideRail();
        expect(getState()).toMatchObject({ sideRailOpen: true, tabRailOpen: true });
    });

    it("is in focus mode exactly when both rails are hidden", () => {
        setState({ sideRailOpen: true, tabRailOpen: true });

        toggleSideRail();
        expect(selectFocusMode(getState())).toBe(false);

        toggleTabRail();
        expect(selectFocusMode(getState())).toBe(true);

        toggleTabRail();
        expect(selectFocusMode(getState())).toBe(false);
    });

    it("entering focus mode hides both rails and leaving it shows both", () => {
        setState({ sideRailOpen: true, tabRailOpen: false });

        toggleZen();
        expect(getState()).toMatchObject({ sideRailOpen: false, tabRailOpen: false });
        expect(selectFocusMode(getState())).toBe(true);

        toggleZen();
        expect(getState()).toMatchObject({ sideRailOpen: true, tabRailOpen: true });
        expect(selectFocusMode(getState())).toBe(false);
    });
});
