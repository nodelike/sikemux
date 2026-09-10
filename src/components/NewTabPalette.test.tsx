import { cleanup, fireEvent, render, screen } from "@testing-library/react";
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
    fireEvent.keyDown(document.activeElement ?? window, { key });
}

function labels(): string[] {
    return screen.getAllByRole("button").map((row) => row.querySelector(".picker-name")?.textContent ?? "");
}

describe("new tab palette", () => {
    it("numbers the destinations a project can open", () => {
        render(<NewTabPalette />);

        expect(labels()).toEqual(["Terminal", "Agent", "Browser", "Editor", "Git", "Search"]);
        expect(screen.getAllByRole("button").map((row) => row.querySelector("kbd")?.textContent)).toEqual(["1", "2", "3", "4", "5", "6"]);
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

    it("opens the full Git workbench on 5", () => {
        render(<NewTabPalette />);

        pressKey("5");

        const roles = (getState().windowsBySession[getState().activeSessionId] ?? []).map((id) => getState().windows[id]?.role);
        expect(roles).toContain("git");
    });

    it("focuses rail search on 6", () => {
        render(<NewTabPalette />);

        pressKey("6");

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

    it("keeps the browser in its fixed slot when unavailable", () => {
        render(<NewTabPalette />);

        expect(screen.getByRole("button", { name: /Browser/ })).toBeDisabled();
        pressKey("3");
        expect(getState().newTabPaletteOpen).toBe(true);
    });

    it("keeps stable slots and explains unavailable project destinations", () => {
        cmd.closeNewTabPalette();
        cmd.createCommandSession();
        cmd.openNewTabPalette();
        render(<NewTabPalette />);

        expect(screen.getAllByRole("button").filter((button) => !(button as HTMLButtonElement).disabled)).toHaveLength(1);
        expect(labels()).toEqual(["Terminal", "Agent", "Browser", "Editor", "Git", "Search"]);
    });
});
