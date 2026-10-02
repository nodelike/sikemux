import { beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi } from "../api/browser";
import { closeDeskItem, openDesk, openDeskTerminal, openFileOnDesk, removeDeskPane, revealDesk, selectDeskItem, toggleDesk } from "./commands";
import { deskEditorId, deskItemsOf, shownDeskItem } from "./desks";
import { taskPtyBindings } from "../tasks/nativeRuntime";
import { collectPanes } from "./layout";
import { agentIdsOf, agentPaneId, shownDeskPaneId } from "./selectors";
import { getState, setState } from "./store";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return { ...actual, browserApi: { ...actual.browserApi, snapshot: vi.fn(), newTab: vi.fn(), closeTab: vi.fn(), closeAgent: vi.fn() } };
});

const initial = getState();

/* An agent pane in a window, which is what a desk gets opened beside. */
function window_() {
    return {
        id: "window",
        name: "1",
        role: "agent" as const,
        root: { type: "pane" as const, id: "agent-1", cwd: "/code", kind: "agent" as const, title: "codex" },
        activePaneId: "agent-1",
    };
}

beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [], activeTabId: null });
    vi.mocked(browserApi.newTab).mockResolvedValue("tab-1");
    setState(initial, true);
    setState({
        sessions: {
            project: {
                id: "project",
                name: "project",
                kind: "project" as const,
                cwd: "/code",
                deploy: null,
                pinned: false,
                activeWindowId: "window",
            },
        },
        sessionOrder: ["project"],
        activeSessionId: "project",
        windows: { window: window_() },
        windowsBySession: { project: ["window"] },
        deskPanes: {},
        desks: {},
        agents: { "agent-1": { id: "agent-1", type: "codex", title: "codex" } },
    } as never);
});

describe("the desk", () => {
    it("opens beside the agent it belongs to, as a leaf in the same window", () => {
        openDesk("agent-1");

        const root = getState().windows.window.root;
        expect(root.type).toBe("split");
        const panes = collectPanes(root);
        expect(panes.map((pane) => pane.kind)).toEqual(["agent", "desk"]);
        const desk = panes[1];
        expect(getState().deskPanes[desk.id]).toBe("agent-1");
        expect(getState().windows.window.activePaneId).toBe(desk.id);
    });

    it("opens once, and focuses the pane it already made", () => {
        openDesk("agent-1");
        const first = collectPanes(getState().windows.window.root)[1].id;
        setState({ windows: { window: { ...getState().windows.window, activePaneId: "agent-1" } } } as never);

        openDesk("agent-1");

        expect(collectPanes(getState().windows.window.root)).toHaveLength(2);
        expect(getState().windows.window.activePaneId).toBe(first);
    });

    it("takes the pane back out when the last thing on it goes", () => {
        openDesk("agent-1");
        const deskId = collectPanes(getState().windows.window.root)[1].id;

        removeDeskPane(deskId);

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent"]);
        expect(getState().deskPanes[deskId]).toBeUndefined();
        expect(getState().windows.window.activePaneId).toBe("agent-1");
    });

    /* A pane that came back from a saved layout has no agent behind it, since
       the link to one only ever lived in memory, and a desk with no agent
       has nothing to hold. */
    it("takes itself back out when it is restored without its agent", () => {
        setState({
            windows: {
                window: {
                    ...window_(),
                    root: {
                        type: "split" as const,
                        id: "split-1",
                        dir: "row" as const,
                        sizes: [0.5, 0.5],
                        children: [window_().root, { type: "pane" as const, id: "orphan", cwd: "/code", kind: "desk" as const, title: "desk" }],
                    },
                    activePaneId: "orphan",
                },
            },
        } as never);

        removeDeskPane("orphan");

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent"]);
        expect(getState().windows.window.activePaneId).toBe("agent-1");
    });

    /* Several things find an agent by reading its window — its tab, its rail row
       and what gets persisted. A desk is a second pane in that window,
       so none of them may go looking at whichever pane happens to be focused. */
    it("keeps the agent findable once its desk is the focused pane", () => {
        openDesk("agent-1");
        const deskId = collectPanes(getState().windows.window.root)[1].id;
        setState({ windows: { window: { ...getState().windows.window, activePaneId: deskId } } } as never);

        expect(agentIdsOf(getState(), "project")).toEqual(["agent-1"]);
        expect(agentPaneId(getState().windows.window)).toBe("agent-1");
    });

    it("does nothing for an agent that is not in any window", () => {
        openDesk("ghost");

        expect(collectPanes(getState().windows.window.root)).toHaveLength(1);
        expect(getState().deskPanes).toEqual({});
    });

    it("hides on a second press of the toggle and leaves the pages open", async () => {
        toggleDesk("agent-1");
        await vi.waitFor(() => expect(browserApi.newTab).toHaveBeenCalledTimes(1));

        toggleDesk("agent-1");

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent"]);
        expect(getState().deskPanes).toEqual({});
        expect(browserApi.closeTab).not.toHaveBeenCalled();
        expect(browserApi.closeAgent).not.toHaveBeenCalled();
    });

    it("shows the tabs it already has instead of opening another", async () => {
        const tab = {
            id: "tab-1",
            title: "Example",
            url: "https://example.com",
            active: true,
            loading: false,
            canGoBack: false,
            canGoForward: false,
            favicon: null,
            acting: false,
        };
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [tab], activeTabId: "tab-1" });

        toggleDesk("agent-1");
        await vi.waitFor(() => expect(browserApi.snapshot).toHaveBeenCalledWith("agent-1"));

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent", "desk"]);
        expect(browserApi.newTab).not.toHaveBeenCalled();
    });

    it("comes on screen for an agent that puts something on it, without taking focus from the agent", () => {
        revealDesk("agent-1");

        const panes = collectPanes(getState().windows.window.root);
        expect(panes.map((pane) => pane.kind)).toEqual(["agent", "desk"]);
        expect(getState().deskPanes[panes[1].id]).toBe("agent-1");
        expect(getState().windows.window.activePaneId).toBe("agent-1");

        revealDesk("agent-1");

        expect(collectPanes(getState().windows.window.root)).toHaveLength(2);
        expect(getState().windows.window.activePaneId).toBe("agent-1");
    });

    it("counts a desk as shown only while its pane is in a window's layout", () => {
        expect(shownDeskPaneId(getState(), "agent-1")).toBeNull();

        openDesk("agent-1");
        const deskId = collectPanes(getState().windows.window.root)[1].id;
        expect(shownDeskPaneId(getState(), "agent-1")).toBe(deskId);

        setState({ windows: { window: window_() } } as never);
        expect(shownDeskPaneId(getState(), "agent-1")).toBeNull();
    });

    it("opens a file at a line, and keeps it once the editor has it", () => {
        openFileOnDesk("agent-1", "/code/src/main.ts", 41, 2);

        const desk = getState().desks["agent-1"];
        expect(desk.active).toBe("file:/code/src/main.ts");
        expect(desk.reveal).toMatchObject({ path: "/code/src/main.ts", line: 41, character: 2 });
        setState({ editorViews: { [deskEditorId("agent-1")]: { openTabs: ["/code/src/main.ts"], activePath: "/code/src/main.ts" } } } as never);
        expect(deskItemsOf(getState(), "agent-1").map((item) => item.key)).toEqual(["file:/code/src/main.ts"]);
    });

    it("puts a task terminal on the desk once, and brings the same one back for a rerun", () => {
        const request = { terminalKey: "task-web", label: "Web", cwd: "/code" };
        const first = openDeskTerminal("agent-1", request);
        const second = openDeskTerminal("agent-1", { ...request, label: "Web again" });

        expect(second).toBe(first);
        expect(getState().desks["agent-1"].terminals).toEqual([{ id: first, terminalKey: "task-web", label: "Web again", cwd: "/code" }]);
        expect(getState().desks["agent-1"].active).toBe(`terminal:${first}`);
        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent", "desk"]);
        expect(getState().windows.window.activePaneId).toBe("agent-1");
    });

    it("lets go of a terminal's process when its tab closes, and shows its neighbour", () => {
        const release = vi.spyOn(taskPtyBindings, "release");
        openFileOnDesk("agent-1", "/code/a.ts");
        setState({ editorViews: { [deskEditorId("agent-1")]: { openTabs: ["/code/a.ts"], activePath: "/code/a.ts" } } } as never);
        const id = openDeskTerminal("agent-1", { terminalKey: "task-web", label: "Web", cwd: "/code" });
        const terminal = deskItemsOf(getState(), "agent-1").find((item) => item.kind === "terminal")!;

        closeDeskItem("agent-1", terminal);

        expect(release).toHaveBeenCalledWith(id);
        expect(getState().desks["agent-1"].terminals).toEqual([]);
        const items = deskItemsOf(getState(), "agent-1");
        expect(shownDeskItem(getState().desks["agent-1"], items)).toBe("file:/code/a.ts");
    });

    it("shows a file that is picked from the strip in the desk's editor", () => {
        openDeskTerminal("agent-1", { terminalKey: "task-web", label: "Web", cwd: "/code" });
        setState({ editorViews: { [deskEditorId("agent-1")]: { openTabs: ["/code/a.ts", "/code/b.ts"], activePath: "/code/a.ts" } } } as never);

        selectDeskItem("agent-1", { key: "file:/code/b.ts", kind: "file", path: "/code/b.ts" });

        expect(getState().desks["agent-1"].active).toBe("file:/code/b.ts");
        expect(getState().editorViews[deskEditorId("agent-1")].activePath).toBe("/code/b.ts");
    });
});
