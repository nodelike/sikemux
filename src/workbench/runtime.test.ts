import { afterEach, describe, expect, it } from "vitest";
import { getState, setState } from "../state/store";
import type { Session, Window } from "../state/types";
import { WorkbenchRuntime } from "./runtime";

const initial = getState();

afterEach(() => setState(initial, true));

describe("WorkbenchRuntime", () => {
    it("reconciles hydrated sessions and disposes removed topology", () => {
        const pane = { type: "pane", id: "pane-runtime", cwd: "/repo", kind: "terminal", title: "shell" } as const;
        const window: Window = { id: "window-runtime", name: "term", role: "term", root: pane, activePaneId: pane.id };
        const session: Session = {
            id: "session-runtime",
            name: "repo",
            kind: "project",
            cwd: "/repo",
            pinned: false,
            activeWindowId: window.id,
        };
        const runtime = new WorkbenchRuntime();
        setState({
            sessions: { [session.id]: session },
            sessionOrder: [session.id],
            activeSessionId: session.id,
            windows: { [window.id]: window },
            windowsBySession: { [session.id]: [window.id] },
        });
        runtime.start();
        expect(runtime.getSnapshot()).toEqual({ sessions: 1, items: 1, reconciliations: 1, started: true });
        expect(runtime.getSession(session.id)?.getSnapshot().activeItemId).toBe(pane.id);

        setState({ diagnosticsOpen: true });
        expect(runtime.getSnapshot().reconciliations).toBe(1);

        setState({ sessions: {}, sessionOrder: [], windows: {}, windowsBySession: {} });
        expect(runtime.getSnapshot()).toEqual({ sessions: 0, items: 0, reconciliations: 2, started: true });
        runtime.stop();
        expect(runtime.getSnapshot()).toEqual({ sessions: 0, items: 0, reconciliations: 2, started: false });
    });

    it("starts with a plugin's pane open, whether or not the plugin is in this build", () => {
        const pane = { type: "pane", id: "pane-plugin", cwd: "", kind: "sikemux.rundeck:deploy", title: "Rundeck" } as const;
        const window: Window = {
            id: "window-plugin",
            name: "Rundeck",
            role: "sikemux.rundeck:deploy",
            root: pane,
            activePaneId: pane.id,
            fixed: true,
        };
        const session: Session = {
            id: "session-plugin",
            name: "Rundeck",
            kind: "sikemux.rundeck:deploy",
            cwd: "",
            pinned: false,
            activeWindowId: window.id,
        };
        setState({
            sessions: { [session.id]: session },
            sessionOrder: [session.id],
            activeSessionId: session.id,
            windows: { [window.id]: window },
            windowsBySession: { [session.id]: [window.id] },
        });
        const runtime = new WorkbenchRuntime();
        expect(() => runtime.start()).not.toThrow();
        expect(runtime.getSession(session.id)?.getSnapshot().activeItemId).toBe(pane.id);
        runtime.stop();
    });
});
