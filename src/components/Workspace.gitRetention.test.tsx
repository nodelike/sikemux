import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useEffect } from "react";
import { Workspace } from "./Workspace";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";

const lifecycle = vi.hoisted(() => ({ mounted: vi.fn(), unmounted: vi.fn() }));
vi.mock("./GitPane", () => ({
    GitPane: ({ active }: { active: boolean }) => {
        useEffect(() => {
            lifecycle.mounted();
            return lifecycle.unmounted;
        }, []);
        return <div data-testid="git-workbench" data-active={active} />;
    },
}));
vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => <div>Terminal</div> }));
const initial = getState();
beforeEach(() => {
    vi.clearAllMocks();
    setState(initial, true);
    cmd.createProjectSession("/work/demo");
});
afterEach(cleanup);

it("keeps Git mounted but inactive between tab switches, and releases it when closed", async () => {
    cmd.openGitWorkbench();
    const state = getState();
    const gitWindow = state.sessions[state.activeSessionId].activeWindowId;
    const { getByTestId } = render(<Workspace />);
    await waitFor(() => expect(lifecycle.mounted).toHaveBeenCalledTimes(1));
    for (let i = 0; i < 3; i++) {
        act(() => cmd.selectWindowByRole("term"));
        expect(getByTestId("git-workbench")).toHaveAttribute("data-active", "false");
        expect(getByTestId("git-workbench").closest('[role="tabpanel"]')).toHaveAttribute("inert");
        act(() => cmd.openGitWorkbench());
        expect(getByTestId("git-workbench")).toHaveAttribute("data-active", "true");
    }
    expect(lifecycle.mounted).toHaveBeenCalledTimes(1);
    expect(lifecycle.unmounted).not.toHaveBeenCalled();
    act(() => cmd.closeWindowById(gitWindow));
    expect(lifecycle.unmounted).toHaveBeenCalledTimes(1);
});
