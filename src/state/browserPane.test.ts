import { beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi } from "../api/browser";
import { closeBrowserPane, openBrowserPane, revealBrowserPane, toggleBrowserPane } from "./commands";
import { collectPanes } from "./layout";
import { agentIdsOf, agentPaneId, shownBrowserPaneId } from "./selectors";
import { getState, setState } from "./store";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return { ...actual, browserApi: { ...actual.browserApi, snapshot: vi.fn(), newTab: vi.fn(), closeTab: vi.fn(), closeAgent: vi.fn() } };
});

const initial = getState();

/* An agent pane in a window, which is what a browser gets opened beside. */
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
        browserPanes: {},
    } as never);
});

describe("the browser pane", () => {
    it("opens beside the agent it belongs to, as a leaf in the same window", () => {
        openBrowserPane("agent-1");

        const root = getState().windows.window.root;
        expect(root.type).toBe("split");
        const panes = collectPanes(root);
        expect(panes.map((pane) => pane.kind)).toEqual(["agent", "browser"]);
        const browser = panes[1];
        expect(getState().browserPanes[browser.id]).toBe("agent-1");
        expect(getState().windows.window.activePaneId).toBe(browser.id);
    });

    it("opens once, and focuses the pane it already made", () => {
        openBrowserPane("agent-1");
        const first = collectPanes(getState().windows.window.root)[1].id;
        setState({ windows: { window: { ...getState().windows.window, activePaneId: "agent-1" } } } as never);

        openBrowserPane("agent-1");

        expect(collectPanes(getState().windows.window.root)).toHaveLength(2);
        expect(getState().windows.window.activePaneId).toBe(first);
    });

    it("takes the pane back out when its last tab goes", () => {
        openBrowserPane("agent-1");
        const browserId = collectPanes(getState().windows.window.root)[1].id;

        closeBrowserPane(browserId);

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent"]);
        expect(getState().browserPanes[browserId]).toBeUndefined();
        expect(getState().windows.window.activePaneId).toBe("agent-1");
    });

    /* A pane that came back from a saved layout has no agent behind it, since
       the link to one only ever lived in memory, and a browser that is not
       running has nothing to draw. */
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
                        children: [window_().root, { type: "pane" as const, id: "orphan", cwd: "/code", kind: "browser" as const, title: "browser" }],
                    },
                    activePaneId: "orphan",
                },
            },
        } as never);

        closeBrowserPane("orphan");

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent"]);
        expect(getState().windows.window.activePaneId).toBe("agent-1");
    });

    /* Several things find an agent by reading its window — its tab, its rail row
       and what gets persisted. A browser pane is a second pane in that window,
       so none of them may go looking at whichever pane happens to be focused. */
    it("keeps the agent findable once its browser is the focused pane", () => {
        openBrowserPane("agent-1");
        const browserId = collectPanes(getState().windows.window.root)[1].id;
        setState({ windows: { window: { ...getState().windows.window, activePaneId: browserId } } } as never);

        expect(agentIdsOf(getState(), "project")).toEqual(["agent-1"]);
        expect(agentPaneId(getState().windows.window)).toBe("agent-1");
    });

    it("does nothing for an agent that is not in any window", () => {
        openBrowserPane("ghost");

        expect(collectPanes(getState().windows.window.root)).toHaveLength(1);
        expect(getState().browserPanes).toEqual({});
    });

    it("leaves the agent reachable from its window while the browser has focus", () => {
        openBrowserPane("agent-1");

        // Opening the browser focuses it, so anything that reads the agent off
        // the focused pane loses the agent, and with it the agent's tab.
        expect(getState().windows.window.activePaneId).not.toBe("agent-1");
        expect(agentPaneId(getState().windows.window)).toBe("agent-1");
        expect(agentIdsOf(getState(), "project")).toEqual(["agent-1"]);
    });

    it("hides on a second press of the toggle and leaves the tabs open", async () => {
        toggleBrowserPane("agent-1");
        await vi.waitFor(() => expect(browserApi.newTab).toHaveBeenCalledTimes(1));

        toggleBrowserPane("agent-1");

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent"]);
        expect(getState().browserPanes).toEqual({});
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

        toggleBrowserPane("agent-1");
        await vi.waitFor(() => expect(browserApi.snapshot).toHaveBeenCalledWith("agent-1"));

        expect(collectPanes(getState().windows.window.root).map((pane) => pane.kind)).toEqual(["agent", "browser"]);
        expect(browserApi.newTab).not.toHaveBeenCalled();
    });

    it("comes on screen for an agent that starts browsing, without taking focus from the agent", () => {
        revealBrowserPane("agent-1");

        const panes = collectPanes(getState().windows.window.root);
        expect(panes.map((pane) => pane.kind)).toEqual(["agent", "browser"]);
        expect(getState().browserPanes[panes[1].id]).toBe("agent-1");
        expect(getState().windows.window.activePaneId).toBe("agent-1");

        revealBrowserPane("agent-1");

        expect(collectPanes(getState().windows.window.root)).toHaveLength(2);
        expect(getState().windows.window.activePaneId).toBe("agent-1");
    });

    it("counts a browser as shown only while its pane is in a window's layout", () => {
        expect(shownBrowserPaneId(getState(), "agent-1")).toBeNull();

        openBrowserPane("agent-1");
        const browserId = collectPanes(getState().windows.window.root)[1].id;
        expect(shownBrowserPaneId(getState(), "agent-1")).toBe(browserId);

        setState({ windows: { window: window_() } } as never);
        expect(shownBrowserPaneId(getState(), "agent-1")).toBeNull();
    });
});
