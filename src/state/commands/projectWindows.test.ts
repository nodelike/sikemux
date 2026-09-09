import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";
import type { LayoutNode, Window } from "../types";

const initial = getState();

beforeEach(() => setState(initial, true));

function projectWindows(): Window[] {
    const state = getState();
    return (state.windowsBySession[state.activeSessionId] ?? []).map((id) => state.windows[id]);
}

/** Rewrites the session's diff window back into the git window it replaced. */
function regressToGitWindow(root?: LayoutNode): string {
    const state = getState();
    const sessionId = state.activeSessionId;
    const diffId = (state.windowsBySession[sessionId] ?? []).find((id) => state.windows[id]?.role === "diff");
    if (!diffId) throw new Error("expected a diff window to regress");
    const win = state.windows[diffId];
    setState({
        windows: {
            ...state.windows,
            [diffId]: { ...win, name: "git", role: "git", root: root ?? { ...win.root, kind: "git", title: "git" } } as Window,
        },
        gitViews: { ...state.gitViews, [diffId]: getState().gitViews[diffId] },
    });
    return diffId;
}

describe("diff window migration", () => {
    it("gives a new project a diff tab and no git tab", () => {
        cmd.createProjectSession("/work/demo");

        const roles = projectWindows().map((win) => win.role);
        expect(roles).toContain("diff");
        expect(roles).not.toContain("git");
    });

    it("converts a single-pane git window in place, keeping its id and position", () => {
        cmd.createProjectSession("/work/demo");
        const before = projectWindows().map((win) => win.id);
        const gitId = regressToGitWindow();

        cmd.ensureDiffWindow();

        const after = projectWindows();
        expect(after.map((win) => win.id)).toEqual(before);
        const migrated = getState().windows[gitId];
        expect(migrated).toMatchObject({ role: "diff", name: "diff" });
        expect(migrated.root).toMatchObject({ type: "pane", kind: "diff" });
        expect(getState().gitViews[gitId]).toBeUndefined();
    });

    it("rebuilds a split git window rather than mislabelling its split root", () => {
        cmd.createProjectSession("/work/demo");
        const gitId = regressToGitWindow({
            type: "split",
            id: "split-1",
            dir: "row",
            sizes: [0.5, 0.5],
            children: [
                { type: "pane", id: "pane-a", cwd: "/work/demo", kind: "git", title: "git" },
                { type: "pane", id: "pane-b", cwd: "/work/demo", kind: "git", title: "git" },
            ],
        });

        cmd.ensureDiffWindow();

        expect(getState().windows[gitId]).toBeUndefined();
        const diffWindows = projectWindows().filter((win) => win.role === "diff");
        expect(diffWindows).toHaveLength(1);
        expect(diffWindows[0].root).toMatchObject({ type: "pane", kind: "diff" });
    });

    it("leaves an already-migrated project untouched", () => {
        cmd.createProjectSession("/work/demo");
        const before = projectWindows();

        cmd.ensureDiffWindow();

        expect(projectWindows()).toEqual(before);
    });

    it("keeps the active window valid when the git tab it pointed at is dropped", () => {
        cmd.createProjectSession("/work/demo");
        const gitId = regressToGitWindow({
            type: "split",
            id: "split-2",
            dir: "row",
            sizes: [0.5, 0.5],
            children: [
                { type: "pane", id: "pane-c", cwd: "/work/demo", kind: "git", title: "git" },
                { type: "pane", id: "pane-d", cwd: "/work/demo", kind: "git", title: "git" },
            ],
        });
        const state = getState();
        setState({ sessions: { ...state.sessions, [state.activeSessionId]: { ...state.sessions[state.activeSessionId], activeWindowId: gitId } } });

        cmd.ensureDiffWindow();

        const session = getState().sessions[getState().activeSessionId];
        expect(getState().windows[session.activeWindowId]).toBeDefined();
    });
});

describe("search tab", () => {
    it("does not give a new project a permanent search tab", () => {
        cmd.createProjectSession("/work/demo");

        expect(projectWindows().map((win) => win.role)).not.toContain("search");
    });

    it("opens a closable search tab on demand and reuses it", () => {
        cmd.createProjectSession("/work/demo");

        cmd.focusGlobalSearch("needle");

        const search = projectWindows().filter((win) => win.role === "search");
        expect(search).toHaveLength(1);
        expect(search[0].fixed).toBeUndefined();
        expect(getState().sessions[getState().activeSessionId].activeWindowId).toBe(search[0].id);

        cmd.focusGlobalSearch();
        expect(projectWindows().filter((win) => win.role === "search")).toHaveLength(1);
    });

    it("drops a search tab restored from an older snapshot", () => {
        cmd.createProjectSession("/work/demo");
        cmd.focusGlobalSearch();
        const searchId = projectWindows().find((win) => win.role === "search")!.id;

        cmd.pruneSearchWindows();

        expect(getState().windows[searchId]).toBeUndefined();
        const session = getState().sessions[getState().activeSessionId];
        expect(getState().windows[session.activeWindowId]).toBeDefined();
    });
});
