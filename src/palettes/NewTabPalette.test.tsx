import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import type { SimStatus } from "../api/sim";
import { NewTabPalette } from "./NewTabPalette";

const simulator = vi.hoisted(() => ({ status: null as SimStatus | null }));
vi.mock("../sim/simStatus", async () => {
    const actual = await vi.importActual<typeof import("../sim/simStatus")>("../sim/simStatus");
    return { ...actual, useSimStatus: () => simulator.status };
});

const initial = getState();

beforeEach(() => {
    vi.restoreAllMocks();
    simulator.status = null;
    setState(initial, true);
    cmd.createProjectSession("/work/demo");
    cmd.openNewTabPalette();
});
afterEach(cleanup);

function pressKey(key: string) {
    fireEvent.keyDown(document.activeElement ?? window, { key });
}

function labels(): string[] {
    return screen.getAllByRole("button").map((row) => row.querySelector(".new-tab-label")?.textContent ?? "");
}

describe("new tab palette", () => {
    it("numbers the destinations a project can open", () => {
        render(<NewTabPalette />);

        expect(labels()).toEqual(["Terminal", "Agent", "Browser", "File", "Git", "Search"]);
        expect(screen.getAllByRole("button").map((row) => row.querySelector("kbd")?.textContent)).toEqual(["1", "2", "3", "4", "5", "6"]);
    });

    it("opens a terminal on 1 and closes itself", () => {
        const before = (getState().windowsBySession[getState().activeSessionId] ?? []).length;
        render(<NewTabPalette />);

        pressKey("1");

        expect((getState().windowsBySession[getState().activeSessionId] ?? []).length).toBe(before + 1);
        expect(getState().newTabPaletteOpen).toBe(false);
    });

    const windowRoles = () => Object.values(getState().windows).map((window) => window.role);

    it.each([
        ["2", "opens the agent picker", () => getState().agentPaletteOpen && !getState().newTabPaletteOpen],
        ["5", "opens the full Git workbench", () => windowRoles().includes("git")],
        ["6", "focuses rail search", () => windowRoles().includes("search")],
    ])("on %s %s", (key, _, opened) => {
        render(<NewTabPalette />);

        pressKey(key);

        expect(opened()).toBe(true);
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

    it("selects the row under a moving mouse, not under a resting one", () => {
        render(<NewTabPalette />);
        const git = screen.getByRole("button", { name: /Git/ });

        fireEvent.mouseEnter(git);
        expect(git).not.toHaveClass("sel");

        fireEvent.mouseMove(window);
        fireEvent.mouseEnter(git);
        expect(git).toHaveClass("sel");
        pressKey("Enter");

        expect(windowRoles()).toContain("git");
    });

    it("offers the iOS Simulator only where it can run", () => {
        simulator.status = { supported: true, installed: false, reason: "Install Xcode to use the iOS Simulator." };
        const { unmount } = render(<NewTabPalette />);
        expect(labels()).not.toContain("iOS Simulator");
        unmount();

        simulator.status = { supported: true, installed: false, reason: null };
        render(<NewTabPalette />);
        expect(labels()).toContain("iOS Simulator");
    });

    it("keeps the browser in its fixed slot when unavailable", () => {
        render(<NewTabPalette />);

        expect(screen.getByRole("button", { name: /Browser/ })).toBeDisabled();
        pressKey("3");
        expect(getState().newTabPaletteOpen).toBe(true);
    });

    it("keeps stable slots, and outside a project offers a terminal or an agent in a project picked next", () => {
        cmd.closeNewTabPalette();
        cmd.createCommandSession();
        cmd.openNewTabPalette();
        render(<NewTabPalette />);

        const enabled = screen.getAllByRole("button").filter((button) => !(button as HTMLButtonElement).disabled);
        expect(enabled.map((row) => row.querySelector(".new-tab-label")?.textContent)).toEqual(["Terminal", "Agent"]);

        pressKey("2");
        expect(getState()).toMatchObject({ pickerOpen: true, pickerMode: "projects" });
    });

    it("puts a browser tab on the desk of the project's agent even from a terminal tab", () => {
        cmd.addAgent("claude");
        const agentWindow = getState().sessions[getState().activeSessionId].activeWindowId;
        cmd.newWindow();
        render(<NewTabPalette />);

        expect(screen.getByRole("button", { name: /Browser/ })).not.toBeDisabled();
        pressKey("3");
        expect(getState().sessions[getState().activeSessionId].activeWindowId).toBe(agentWindow);
    });
});
