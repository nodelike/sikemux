import { afterEach, beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";
import { currentPlace } from "./places";

const initial = getState();
let stop: () => void = () => {};

beforeEach(() => {
    setState(initial, true);
    cmd.createProjectSession("/work/app");
    stop = cmd.watchPlaces();
});

afterEach(() => stop());

const activeTab = (): string => {
    const state = getState();
    return state.sessions[state.activeSessionId]?.activeWindowId ?? "";
};

/** Opens a terminal tab and returns its id. */
function newTab(): string {
    cmd.newWindow();
    return activeTab();
}

describe("going back and forward through places", () => {
    it("goes back through the tabs the person was in, and forward again", () => {
        const first = activeTab();
        const second = newTab();
        const third = newTab();
        expect(cmd.placeHistory()).toEqual({ canGoBack: true, canGoForward: false });

        cmd.goBack();
        expect(activeTab()).toBe(second);
        cmd.goBack();
        expect(activeTab()).toBe(first);
        expect(cmd.placeHistory()).toEqual({ canGoBack: false, canGoForward: true });

        cmd.goForward();
        expect(activeTab()).toBe(second);
        cmd.goForward();
        expect(activeTab()).toBe(third);
        expect(cmd.placeHistory()).toEqual({ canGoBack: true, canGoForward: false });
    });

    it("forgets the way forward once the person goes somewhere new", () => {
        const first = activeTab();
        newTab();
        cmd.goBack();
        expect(cmd.placeHistory().canGoForward).toBe(true);

        const elsewhere = newTab();
        expect(cmd.placeHistory()).toEqual({ canGoBack: true, canGoForward: false });
        cmd.goBack();
        expect(activeTab()).toBe(first);
        cmd.goForward();
        expect(activeTab()).toBe(elsewhere);
    });

    it("skips a tab that has since closed", () => {
        const first = activeTab();
        const closing = newTab();
        newTab();
        cmd.closeWindowById(closing);

        cmd.goBack();
        expect(activeTab()).toBe(first);
        expect(cmd.placeHistory().canGoBack).toBe(false);
    });

    it("goes back to another project, and to the file the editor showed", () => {
        const appSession = getState().activeSessionId;
        cmd.requestOpenFile("/work/app/src/main.ts");
        expect(currentPlace(getState())?.doc).toBe("/work/app/src/main.ts");
        const pane = getState().windows[activeTab()]?.activePaneId ?? "";
        cmd.openEditorTab(pane, "/work/app/src/util.ts");
        expect(currentPlace(getState())?.doc).toBe("/work/app/src/util.ts");

        cmd.createProjectSession("/work/site");
        expect(getState().activeSessionId).not.toBe(appSession);

        cmd.goBack();
        expect(getState().activeSessionId).toBe(appSession);
        expect(currentPlace(getState())?.doc).toBe("/work/app/src/util.ts");
        cmd.goBack();
        expect(currentPlace(getState())?.doc).toBe("/work/app/src/main.ts");
    });

    it("does nothing with nowhere to go", () => {
        const only = activeTab();
        cmd.goBack();
        cmd.goForward();
        expect(activeTab()).toBe(only);
        expect(cmd.placeHistory()).toEqual({ canGoBack: false, canGoForward: false });
    });
});
