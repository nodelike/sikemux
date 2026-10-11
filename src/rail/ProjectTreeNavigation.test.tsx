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

it("opens and reuses Files, Git and Search from the expanded project tree", () => {
    render(<SideRail />);
    for (const [label, role] of [
        ["Files", "files"],
        ["Git", "git"],
        ["Search", "search"],
    ]) {
        fireEvent.click(screen.getByRole("button", { name: label }));
        expect(activeRole()).toBe(role);
        fireEvent.click(screen.getByRole("button", { name: label }));
        expect(Object.values(getState().windows).filter((window) => window.role === role)).toHaveLength(1);
    }
});
