import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import { SideRail } from "./SideRail";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";

const initial = getState();
beforeEach(() => {
    setState(initial, true);
    cmd.createProjectSession("/work/demo");
});
afterEach(cleanup);

function activeRole() {
    const state = getState();
    return state.windows[state.sessions[state.activeSessionId].activeWindowId].role;
}

it("toggles the file tree from Files without opening a window", () => {
    setState({ fileTreeOpen: false });
    render(<SideRail />);
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    expect(getState().fileTreeOpen).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    expect(getState().fileTreeOpen).toBe(false);
    expect(Object.values(getState().windows).some((window) => window.role === "files")).toBe(false);
});

it("opens and reuses Git and Search from the expanded project tree", () => {
    render(<SideRail />);
    for (const [label, role] of [
        ["Git", "git"],
        ["Search", "search"],
    ]) {
        fireEvent.click(screen.getByRole("button", { name: label }));
        expect(activeRole()).toBe(role);
        fireEvent.click(screen.getByRole("button", { name: label }));
        expect(Object.values(getState().windows).filter((window) => window.role === role)).toHaveLength(1);
    }
});
