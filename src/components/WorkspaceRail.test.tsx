import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkspaceRail } from "./WorkspaceRail";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";

vi.mock("./AgentRail", () => ({ AgentRailBody: () => <div>Agent list</div> }));
vi.mock("./FileTree", () => ({ FileTree: () => <div>File tree</div> }));
vi.mock("./SearchPane", () => ({ SearchPane: () => <div>Search files</div> }));
vi.mock("./rail/RailChanges", () => ({ RailChanges: () => <div>Sidebar Git controls</div> }));

const initial = getState();
beforeEach(() => {
    setState(initial, true);
    cmd.createProjectSession("/work/demo");
    cmd.setRailTab("changes");
});
afterEach(cleanup);

describe("Git sidebar visibility", () => {
    it("collapses duplicate controls in the Git workbench and restores them for diffs", () => {
        render(<WorkspaceRail />);
        expect(screen.getByText("Sidebar Git controls")).toBeInTheDocument();
        act(() => cmd.openGitWorkbench());
        expect(screen.queryByText("Sidebar Git controls")).not.toBeInTheDocument();
        expect(document.querySelector("aside")).toHaveClass("workspace-rail-git-collapsed");
        expect(screen.getByRole("tab", { name: "Changes" })).toBeVisible();
        act(() => cmd.openDiff("src/file.ts"));
        expect(screen.getByText("Sidebar Git controls")).toBeInTheDocument();
        expect(document.querySelector("aside")).not.toHaveClass("workspace-rail-git-collapsed");
    });

    it("keeps other sidebar panels accessible alongside the workbench", () => {
        cmd.openGitWorkbench();
        render(<WorkspaceRail />);
        fireEvent.click(screen.getByRole("tab", { name: "Files" }));
        expect(screen.getByText("File tree")).toBeInTheDocument();
        expect(document.querySelector("aside")).not.toHaveClass("workspace-rail-git-collapsed");
        fireEvent.click(screen.getByRole("tab", { name: "Changes" }));
        expect(screen.queryByText("Sidebar Git controls")).not.toBeInTheDocument();
    });

    it("restores the sidebar when switching projects or showing an agent", () => {
        cmd.openGitWorkbench();
        render(<WorkspaceRail />);
        act(() => {
            const state = getState();
            const session = state.sessions[state.activeSessionId];
            setState({ sessions: { ...state.sessions, [session.id]: { ...session, view: "agent" } } });
        });
        expect(screen.getByText("Sidebar Git controls")).toBeInTheDocument();
        act(() => cmd.createProjectSession("/work/other"));
        expect(screen.getByText("Sidebar Git controls")).toBeInTheDocument();
    });
});
