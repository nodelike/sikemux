import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { NewTabPalette } from "./NewTabPalette";

const initial = getState();

beforeEach(() => {
    vi.restoreAllMocks();
    setState(initial, true);
    cmd.createProjectSession("/work/demo");
    cmd.openNewTabPalette();
});
afterEach(cleanup);

function pressKey(key: string) {
    window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

function labels(): string[] {
    return screen.getAllByRole("button").map((row) => row.querySelector(".new-tab-label")?.textContent ?? "");
}

describe("new tab palette", () => {
    it("numbers the destinations a project can open", () => {
        render(<NewTabPalette />);

        expect(labels()).toEqual(["Terminal", "Agent", "Editor", "Diff", "Search"]);
        expect(screen.getAllByRole("button").map((row) => row.querySelector(".new-tab-key")?.textContent)).toEqual(["1", "2", "3", "4", "5"]);
    });

    it("opens a terminal on 1 and closes itself", () => {
        const before = (getState().windowsBySession[getState().activeSessionId] ?? []).length;
        render(<NewTabPalette />);

        pressKey("1");

        expect((getState().windowsBySession[getState().activeSessionId] ?? []).length).toBe(before + 1);
        expect(getState().newTabPaletteOpen).toBe(false);
    });

    it("opens the agent picker on 2", () => {
        render(<NewTabPalette />);

        pressKey("2");

        expect(getState().agentPaletteOpen).toBe(true);
        expect(getState().newTabPaletteOpen).toBe(false);
    });

    it("opens the diff tab on 4", () => {
        render(<NewTabPalette />);

        pressKey("4");

        const roles = (getState().windowsBySession[getState().activeSessionId] ?? []).map((id) => getState().windows[id]?.role);
        expect(roles).toContain("diff");
    });

    it("focuses rail search on 5", () => {
        render(<NewTabPalette />);

        pressKey("5");

        expect(getState().railTab).toBe("search");
    });

    it("ignores a digit past the end of the list", () => {
        render(<NewTabPalette />);

        pressKey("9");

        expect(getState().newTabPaletteOpen).toBe(true);
    });

    it("closes on Escape without opening anything", () => {
        const before = (getState().windowsBySession[getState().activeSessionId] ?? []).length;
        render(<NewTabPalette />);

        pressKey("Escape");

        expect(getState().newTabPaletteOpen).toBe(false);
        expect((getState().windowsBySession[getState().activeSessionId] ?? []).length).toBe(before);
    });

    it("moves the selection with arrows and opens it with Enter", () => {
        render(<NewTabPalette />);

        pressKey("ArrowDown");
        pressKey("Enter");

        expect(getState().agentPaletteOpen).toBe(true);
    });

    it("omits the browser when no agent is running to host it", () => {
        render(<NewTabPalette />);

        expect(labels()).not.toContain("Browser");
    });

    it("offers only a terminal outside a project", () => {
        cmd.closeNewTabPalette();
        cmd.createCommandSession();
        cmd.openNewTabPalette();
        render(<NewTabPalette />);

        expect(labels()).toEqual(["Terminal"]);
    });
});
