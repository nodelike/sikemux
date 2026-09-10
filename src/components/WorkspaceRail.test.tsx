import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkspaceRail } from "./WorkspaceRail";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";

vi.mock("./AgentRail", () => ({ AgentRailBody: () => <div>Agent list</div> }));
vi.mock("./FileTree", () => ({ FileTree: () => <div>File tree</div> }));
vi.mock("./SearchPane", () => ({ SearchPane: () => <div>Search files</div> }));

const initial = getState();
beforeEach(() => {
    setState(initial, true);
    cmd.createProjectSession("/work/demo");
});
afterEach(cleanup);

function gitWindows() {
    const state = getState();
    return state.windowsBySession[state.activeSessionId].map((id) => state.windows[id]).filter((win) => win.role === "git");
}

function activeRole() {
    const state = getState();
    return state.windows[state.sessions[state.activeSessionId].activeWindowId].role;
}

describe("single Git workbench", () => {
    it("opens and reuses the workbench from the Git icon without a sidebar panel", () => {
        render(<WorkspaceRail />);
        fireEvent.click(screen.getByRole("tab", { name: "Git" }));
        expect(activeRole()).toBe("git");
        expect(gitWindows()).toHaveLength(1);
        expect(screen.queryByRole("tabpanel")).not.toBeInTheDocument();
        expect(screen.queryByText("More Git tools")).not.toBeInTheDocument();
        act(() => cmd.openDiff("src/file.ts"));
        expect(screen.queryByRole("tabpanel")).not.toBeInTheDocument();
        fireEvent.click(screen.getByRole("tab", { name: "Git" }));
        expect(activeRole()).toBe("git");
        expect(gitWindows()).toHaveLength(1);
    });

    it("keeps Files, Agents and Search accessible", () => {
        cmd.setRailTab("changes");
        render(<WorkspaceRail />);
        for (const [label, content] of [
            ["Files", "File tree"],
            ["Agents", "Agent list"],
            ["Search", "Search files"],
        ]) {
            fireEvent.click(screen.getByRole("tab", { name: label }));
            expect(screen.getByText(content)).toBeInTheDocument();
            expect(document.querySelector("aside")).not.toHaveClass("workspace-rail-git-collapsed");
        }
    });

    it("does not restore duplicate controls after project switches or saved Changes selection", () => {
        setState({ railTab: "changes" });
        render(<WorkspaceRail />);
        expect(screen.queryByRole("tabpanel")).not.toBeInTheDocument();
        act(() => cmd.createProjectSession("/work/other"));
        expect(screen.queryByRole("tabpanel")).not.toBeInTheDocument();
        fireEvent.click(screen.getByRole("tab", { name: "Git" }));
        expect(activeRole()).toBe("git");
        expect(gitWindows()[0].root).toMatchObject({ cwd: "/work/other", kind: "git" });
    });

    it("uses the same workbench for the top bar and palette entry points", () => {
        cmd.openGitPane();
        cmd.openGitWorkbench();
        cmd.setRailTab("changes");
        expect(gitWindows()).toHaveLength(1);
        expect(activeRole()).toBe("git");
    });
});
