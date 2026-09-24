import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi, type BrowserSnapshot, type BrowserTab } from "../api/browser";
import { browserPaneView, refreshBrowserStrip, takeBrowserRestore, useBrowserStrips } from "./browserStrips";
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
        setState({ browserPanes: { "pane-a": "agent-one", "pane-b": "agent-two" } } as never);
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
});

describe("what a browser pane saves", () => {
    beforeEach(() => {
        setState({ browserPanes: { "pane-browser": "agent-one" } } as never);
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

        expect(browserPaneView("pane-browser")).toEqual({
            agentId: "agent-one",
            tabs: [
                { url: "https://example.com/docs", title: "Docs" },
                { url: "https://news.test", title: "News" },
            ],
            activeIndex: 1,
        });
    });

    it("saves nothing for a pane with no agent, no strip, or only a blank tab", () => {
        expect(browserPaneView("pane-browser")).toBeNull();
        setState({ browserStrips: { "agent-one": strip([tab({ url: "about:blank", title: "" })], "tab-one") } } as never);
        expect(browserPaneView("pane-browser")).toBeNull();
        setState({ browserPanes: {} } as never);
        expect(browserPaneView("pane-browser")).toBeNull();
    });

    /* A restored pane nobody opened has no strip of its own yet, and losing
       the tabs it is holding would mean a second restart threw them away. */
    it("keeps holding the tabs a restored pane has not opened yet", () => {
        const pending = { agentId: "agent-one", tabs: [{ url: "https://held.test", title: "Held" }], activeIndex: 0 };
        setState({ browserRestores: { "pane-browser": pending }, browserStrips: { "agent-one": strip([], null) } } as never);

        expect(browserPaneView("pane-browser")).toEqual(pending);

        expect(takeBrowserRestore("pane-browser")).toEqual(pending);
        expect(takeBrowserRestore("pane-browser")).toBeNull();
        expect(browserPaneView("pane-browser")).toBeNull();
    });
});
