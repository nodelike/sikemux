import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";
import { subscribe } from "../bus";
import type { PaneKind, Window, WindowRole } from "../types";

const initial = getState();

beforeEach(() => setState(initial, true));

function projectWindows(): Window[] {
    const state = getState();
    return (state.windowsBySession[state.activeSessionId] ?? []).map((id) => state.windows[id]);
}

function roles(): WindowRole[] {
    return projectWindows().map((win) => win.role);
}

/** Rebuilds the fixed tab an older session would have persisted for `role`. */
function addLegacyFixedWindow(role: WindowRole, kind: PaneKind, openTabs: string[] = []): string {
    const state = getState();
    const sessionId = state.activeSessionId;
    const paneId = `pane-${role}`;
    const windowId = `win-${role}`;
    setState({
        windows: {
            ...state.windows,
            [windowId]: {
                id: windowId,
                name: role,
                role,
                fixed: true,
                activePaneId: paneId,
                root: { type: "pane", id: paneId, cwd: "/work/demo", kind, title: role },
            },
        },
        windowsBySession: { ...state.windowsBySession, [sessionId]: [...(state.windowsBySession[sessionId] ?? []), windowId] },
        ...(openTabs.length > 0 ? { editorViews: { ...state.editorViews, [paneId]: { openTabs, activePath: openTabs[0] } } } : {}),
    });
    return windowId;
}

describe("project windows", () => {
    it("starts a project with only its terminal", () => {
        cmd.createProjectSession("/work/demo");

        expect(roles()).toEqual(["term"]);
    });

    it.each([
        ["editor", () => cmd.openEditorPane(), "files"],
        ["diff", () => cmd.openDiffPane(), "diff"],
    ])("opens the %s tab on demand as a closable tab, and reuses it", (_label, open, role) => {
        cmd.createProjectSession("/work/demo");

        open();

        const matching = projectWindows().filter((win) => win.role === role);
        expect(matching).toHaveLength(1);
        expect(matching[0].fixed).toBeUndefined();
        expect(getState().sessions[getState().activeSessionId].activeWindowId).toBe(matching[0].id);

        open();
        expect(projectWindows().filter((win) => win.role === role)).toHaveLength(1);
    });

    it("opens a file into a fresh editor tab by seeding the view the pane hydrates from", () => {
        cmd.createProjectSession("/work/demo");

        cmd.requestOpenFile("/work/demo/src/main.ts");

        const editor = projectWindows().find((win) => win.role === "files");
        expect(editor).toBeDefined();
        expect(getState().editorViews[editor!.activePaneId]).toEqual({
            openTabs: ["/work/demo/src/main.ts"],
            activePath: "/work/demo/src/main.ts",
        });
    });

    it("does not re-seed an editor that is already open", () => {
        cmd.createProjectSession("/work/demo");
        cmd.requestOpenFile("/work/demo/a.ts");
        const editor = projectWindows().find((win) => win.role === "files")!;

        cmd.requestOpenFile("/work/demo/b.ts");

        expect(projectWindows().filter((win) => win.role === "files")).toHaveLength(1);
        expect(getState().editorViews[editor.activePaneId].openTabs).toEqual(["/work/demo/a.ts"]);
    });

    it("opens search in the content area", () => {
        cmd.createProjectSession("/work/demo");

        cmd.focusGlobalSearch("needle");

        expect(roles()).toContain("search");
    });

    it("points the diff tab at the file whose changes were clicked", () => {
        cmd.createProjectSession("/work/demo");

        cmd.openDiff("src/state/commands.ts");

        expect(getState().diffTarget["/work/demo"]).toEqual({ kind: "worktree", path: "src/state/commands.ts" });
        expect(roles()).toContain("diff");
    });

    it("points the diff tab at a commit when one is selected", () => {
        cmd.createProjectSession("/work/demo");

        cmd.openCommitDiff("3984bec", "reuse GitGraph");

        expect(getState().diffTarget["/work/demo"]).toEqual({ kind: "commit", rev: "3984bec", subject: "reuse GitGraph" });
    });
});

describe("pruning legacy fixed tabs", () => {
    it.each([
        ["diff", "diff"],
        ["search", "search"],
        ["files", "editor"],
    ])("drops the fixed %s tab a saved session was built with", (role, kind) => {
        cmd.createProjectSession("/work/demo");
        const windowId = addLegacyFixedWindow(role as WindowRole, kind as PaneKind);

        cmd.pruneOnDemandWindows();

        expect(getState().windows[windowId]).toBeUndefined();
        expect(roles()).toEqual(["term"]);
    });

    it("keeps an editor holding open files, but makes it closable", () => {
        cmd.createProjectSession("/work/demo");
        const windowId = addLegacyFixedWindow("files", "editor", ["/work/demo/keep.ts"]);

        cmd.pruneOnDemandWindows();

        const kept = getState().windows[windowId];
        expect(kept).toBeDefined();
        expect(kept.fixed).toBeUndefined();
        expect(getState().editorViews[kept.activePaneId].openTabs).toEqual(["/work/demo/keep.ts"]);
    });

    it("keeps the active window valid when the tab it pointed at is dropped", () => {
        cmd.createProjectSession("/work/demo");
        const windowId = addLegacyFixedWindow("diff", "diff");
        const state = getState();
        setState({
            sessions: { ...state.sessions, [state.activeSessionId]: { ...state.sessions[state.activeSessionId], activeWindowId: windowId } },
        });

        cmd.pruneOnDemandWindows();

        const session = getState().sessions[getState().activeSessionId];
        expect(getState().windows[session.activeWindowId]).toBeDefined();
    });

    it("leaves an already-pruned project untouched", () => {
        cmd.createProjectSession("/work/demo");
        const before = projectWindows();

        cmd.pruneOnDemandWindows();

        expect(projectWindows()).toEqual(before);
    });
});

describe("file tabs in the session strip", () => {
    function editorPaneId(): string {
        const editor = projectWindows().find((win) => win.role === "files");
        if (!editor) throw new Error("expected an editor window");
        return editor.activePaneId;
    }

    it("switches the editor's document when its tab is selected", () => {
        cmd.createProjectSession("/work/demo");
        cmd.requestOpenFile("/work/demo/a.ts");
        const paneId = editorPaneId();
        cmd.openEditorTab(paneId, "/work/demo/b.ts");
        const windowId = projectWindows().find((win) => win.role === "files")!.id;

        cmd.selectTab({ kind: "file", id: windowId, path: "/work/demo/a.ts" });

        expect(getState().editorViews[paneId].activePath).toBe("/work/demo/a.ts");
        expect(getState().sessions[getState().activeSessionId].activeWindowId).toBe(windowId);
    });

    it("asks the editor to close a document rather than closing it behind its back", () => {
        cmd.createProjectSession("/work/demo");
        cmd.requestOpenFile("/work/demo/a.ts");
        const paneId = editorPaneId();
        const windowId = projectWindows().find((win) => win.role === "files")!.id;
        const seen: { paneId: string; path: string }[] = [];
        const stop = subscribe("close-file", (event) => seen.push({ paneId: event.paneId, path: event.path }));

        cmd.closeTab({ kind: "file", id: windowId, path: "/work/demo/a.ts" });
        stop();

        // The editor owns the unsaved-changes prompt, so the tab survives until
        // it decides; the command only asks.
        expect(seen).toEqual([{ paneId, path: "/work/demo/a.ts" }]);
        expect(getState().editorViews[paneId].openTabs).toEqual(["/work/demo/a.ts"]);
    });
});
