import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi, type BrowserSnapshot, type BrowserTab } from "../api/browser";
import { refreshBrowserStrip, useBrowserStrips } from "./browserStrips";
import { deskEditorId, deskView, takeDeskRestore } from "./desks";
import { getState, setState } from "./store";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return { ...actual, browserApi: { snapshot: vi.fn(), subscribeTabs: vi.fn() } };
});

function tab(overrides: Partial<BrowserTab> = {}): BrowserTab {
    return {
        id: "tab-one",
        title: "Example",
        url: "https://example.com",
        active: true,
        loading: false,
        canGoBack: false,
        canGoForward: false,
        favicon: null,
        acting: false,
        ...overrides,
    };
}

const strip = (tabs: BrowserTab[], activeTabId: string | null): BrowserSnapshot => ({ tabs, activeTabId });

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    vi.mocked(browserApi.snapshot).mockResolvedValue(strip([tab()], "tab-one"));
    vi.mocked(browserApi.subscribeTabs).mockResolvedValue(vi.fn());
});

afterEach(() => {
    vi.clearAllMocks();
});

describe("the app's reader of the browser strips", () => {
    it("reads for every agent with a pane, whether or not anyone is looking at it", async () => {
        setState({ deskPanes: { "pane-a": "agent-one", "pane-b": "agent-two" } } as never);
        let announce = () => {};
        vi.mocked(browserApi.subscribeTabs).mockImplementation(async (listener) => {
            announce = listener;
            return vi.fn<() => void>();
        });

        const { unmount } = renderHook(() => useBrowserStrips());
        await vi.waitFor(() => expect(getState().browserStrips["agent-two"]).toBeDefined());
        vi.mocked(browserApi.snapshot).mockResolvedValue(strip([tab({ url: "https://moved.test" })], "tab-one"));
        announce();

        await vi.waitFor(() => expect(getState().browserStrips["agent-one"].tabs[0].url).toBe("https://moved.test"));
        unmount();
    });

    /* A loading page reports itself several times over, and each report is an
       IPC round trip, so a burst has to collapse rather than queue. */
    it("collapses a burst of reports into the read in flight and one after it", async () => {
        let release: (() => void) | undefined;
        vi.mocked(browserApi.snapshot).mockImplementation(() => new Promise((resolve) => (release = () => resolve(strip([tab()], "tab-one")))));

        const first = refreshBrowserStrip("agent-one");
        await vi.waitFor(() => expect(release).toBeDefined());
        void refreshBrowserStrip("agent-one");
        void refreshBrowserStrip("agent-one");
        release!();
        await vi.waitFor(() => expect(browserApi.snapshot).toHaveBeenCalledTimes(2));
        release!();
        await first;

        expect(browserApi.snapshot).toHaveBeenCalledTimes(2);
    });

    it("adds new pages to the end of the desk and brings the page the agent moved to forward", async () => {
        setState({
            browserStrips: { "agent-one": strip([], null) },
            desks: { "agent-one": { order: ["file:/repo/a.ts"], active: "file:/repo/a.ts", terminals: [], simulators: [], reveal: null } },
        } as never);
        await refreshBrowserStrip("agent-one");
        expect(getState().desks["agent-one"]).toMatchObject({ order: ["file:/repo/a.ts", "browser:tab-one"], active: "browser" });

        setState({ desks: { "agent-one": { ...getState().desks["agent-one"], active: "file:/repo/a.ts" } } } as never);
        await refreshBrowserStrip("agent-one");
        expect(getState().desks["agent-one"].active).toBe("file:/repo/a.ts");

        vi.mocked(browserApi.snapshot).mockResolvedValue(strip([tab(), tab({ id: "tab-two" })], "tab-two"));
        await refreshBrowserStrip("agent-one");
        expect(getState().desks["agent-one"]).toMatchObject({ order: ["file:/repo/a.ts", "browser:tab-one", "browser:tab-two"], active: "browser" });
    });
});

describe("what a desk saves", () => {
    beforeEach(() => {
        setState({ deskPanes: { "pane-desk": "agent-one" } } as never);
    });

    it("keeps the pages, drops the blank tab, and remembers which was in front", () => {
        setState({
            browserStrips: {
                "agent-one": strip(
                    [
                        tab({ id: "tab-blank", url: "about:blank", title: "" }),
                        tab({ id: "tab-docs", url: "https://example.com/docs", title: "Docs" }),
                        tab({ id: "tab-news", url: "https://news.test", title: "News" }),
                    ],
                    "tab-news",
                ),
            },
        } as never);

        expect(deskView("pane-desk")).toEqual({
            agentId: "agent-one",
            tabs: [
                { url: "https://example.com/docs", title: "Docs" },
                { url: "https://news.test", title: "News" },
            ],
            activeIndex: 1,
            files: [],
        });
    });

    it("keeps the files that are open on the desk, with or without pages", () => {
        setState({ editorViews: { [deskEditorId("agent-one")]: { openTabs: ["/repo/a.ts"], activePath: "/repo/a.ts" } } } as never);

        expect(deskView("pane-desk")).toEqual({ agentId: "agent-one", tabs: [], activeIndex: 0, files: ["/repo/a.ts"] });
    });

    it("saves nothing for a desk with no agent, no strip, or only a blank tab", () => {
        expect(deskView("pane-desk")).toBeNull();
        setState({ browserStrips: { "agent-one": strip([tab({ url: "about:blank", title: "" })], "tab-one") } } as never);
        expect(deskView("pane-desk")).toBeNull();
        setState({ deskPanes: {} } as never);
        expect(deskView("pane-desk")).toBeNull();
    });

    /* A restored desk nobody opened has no strip of its own yet, and losing
       the pages it is holding would mean a second restart threw them away. */
    it("keeps holding the pages a restored desk has not opened yet", () => {
        const pending = { agentId: "agent-one", tabs: [{ url: "https://held.test", title: "Held" }], activeIndex: 0, files: [] };
        setState({ deskRestores: { "pane-desk": pending }, browserStrips: { "agent-one": strip([], null) } } as never);

        expect(deskView("pane-desk")).toEqual(pending);

        expect(takeDeskRestore("pane-desk")).toEqual(pending);
        expect(takeDeskRestore("pane-desk")).toBeNull();
        expect(deskView("pane-desk")).toBeNull();
    });
});
