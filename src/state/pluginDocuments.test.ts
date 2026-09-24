import { beforeEach, describe, expect, it } from "vitest";
import { create } from "zustand";
import { registerFrontendPlugin } from "../plugins/registry";
import * as cmd from "./commands";
import { activeTabRef, selectTabRefs } from "./selectors";
import { getState, setState } from "./store";

const PAD = "test.docs:pad";
const pads = create<{ open: string[]; active: string | null }>(() => ({ open: [], active: null }));

registerFrontendPlugin({
    id: "test.docs",
    surfaces: [
        {
            kind: PAD,
            title: "Pad",
            icon: () => null,
            render: () => null,
            documents: {
                list: () => ({ ids: pads.getState().open, activeId: pads.getState().active }),
                describe: (_pane, id) => ({ label: id.toUpperCase(), dirty: id === "b" }),
                select: (_pane, id) => pads.setState({ active: id }),
                close: (_pane, id) =>
                    pads.setState((state) => {
                        const open = state.open.filter((doc) => doc !== id);
                        return { open, active: state.active === id ? (open[0] ?? null) : state.active };
                    }),
                reorder: (_pane, source, target, placement) =>
                    pads.setState((state) => {
                        const open = state.open.filter((doc) => doc !== source);
                        const at = open.indexOf(target) + (placement === "after" ? 1 : 0);
                        open.splice(at, 0, source);
                        return { open };
                    }),
                subscribe: (listener) => pads.subscribe(listener),
            },
        },
    ],
    open: () => cmd.openPluginSession(PAD),
    openTitle: "Open Pad",
});

const initial = getState();

function openPad(open: string[], active: string | null) {
    setState(initial, true);
    pads.setState({ open, active });
    cmd.openPluginSession(PAD);
    const session = getState().sessions[getState().activeSessionId];
    return { session, windowId: session.activeWindowId };
}

beforeEach(() => setState(initial, true));

describe("a plugin's documents in the workspace strip", () => {
    it("give its window a tab per open document, and none when nothing is open", () => {
        const { session, windowId } = openPad(["a", "b", "c"], "b");
        expect(selectTabRefs(getState(), session.id)).toEqual([
            { id: windowId, doc: "a" },
            { id: windowId, doc: "b" },
            { id: windowId, doc: "c" },
        ]);
        expect(activeTabRef(getState().sessions[session.id], getState().windows)).toEqual({ id: windowId, doc: "b" });

        pads.setState({ open: [], active: null });
        expect(selectTabRefs(getState(), session.id)).toEqual([]);
        expect(activeTabRef(getState().sessions[session.id], getState().windows)).toEqual({ id: windowId });
    });

    it("select, cycle and close through the plugin", () => {
        const { windowId } = openPad(["a", "b", "c"], "a");
        cmd.selectTab({ id: windowId, doc: "c" });
        expect(pads.getState().active).toBe("c");

        cmd.cycleTabs(1);
        expect(pads.getState().active).toBe("a");
        cmd.cycleTabs(-1);
        expect(pads.getState().active).toBe("c");

        cmd.closeTab({ id: windowId, doc: "b" });
        expect(pads.getState().open).toEqual(["a", "c"]);
    });

    it("close the document in front on the close shortcut, never the plugin's session", () => {
        const { session } = openPad(["a", "b"], "b");
        cmd.closeActiveFocusTarget();
        expect(pads.getState()).toEqual({ open: ["a"], active: "a" });
        expect(getState().sessions[session.id]).toBeDefined();
    });

    it("reorder within the plugin's own list", () => {
        const { windowId } = openPad(["a", "b", "c"], "a");
        cmd.reorderDocumentTab(windowId, "c", "a", "before");
        expect(pads.getState().open).toEqual(["c", "a", "b"]);
    });
});
