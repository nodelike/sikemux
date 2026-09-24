import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi, type BrowserSnapshot, type BrowserTab } from "../api/browser";
import { occludeNativeViews, useStageMotion } from "../state/nativeViews";
import { useToasts } from "../state/toast";
import { getState, setState } from "../state/store";
import { BrowserPaneHost } from "./BrowserPane";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return {
        ...actual,
        browserApi: {
            snapshot: vi.fn(),
            newTab: vi.fn(),
            closeAgent: vi.fn(),
            switchTab: vi.fn(),
            closeTab: vi.fn(),
            navigate: vi.fn(),
            back: vi.fn(),
            forward: vi.fn(),
            reload: vi.fn(),
            setBounds: vi.fn(),
            subscribeTabs: vi.fn(),
            subscribeShortcuts: vi.fn(),
        },
    };
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

const snapshot: BrowserSnapshot = {
    tabs: [tab()],
    activeTabId: "tab-one",
};

/** A pane's worth of saved tabs, as hydration would hand them over. */
const restored = {
    agentId: "agent-one",
    tabs: [
        { url: "https://example.com", title: "Example" },
        { url: "https://second.test", title: "Second" },
    ],
    activeIndex: 1,
};

let resizeCallbacks: Array<() => void> = [];

beforeEach(() => {
    resizeCallbacks = [];
    vi.stubGlobal(
        "ResizeObserver",
        class {
            constructor(callback: () => void) {
                resizeCallbacks.push(callback);
            }
            observe() {}
            disconnect() {}
        },
    );
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
        callback(0);
        return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
        left: 640,
        top: 96,
        width: 480.4,
        height: 320.6,
        right: 0,
        bottom: 0,
        x: 640,
        y: 96,
        toJSON: () => ({}),
    });
    setState({ browserStrips: {}, browserRestores: {} } as never);
    vi.mocked(browserApi.snapshot).mockResolvedValue(snapshot);
    vi.mocked(browserApi.subscribeTabs).mockResolvedValue(vi.fn());
    for (const operation of [
        browserApi.newTab,
        browserApi.closeAgent,
        browserApi.switchTab,
        browserApi.closeTab,
        browserApi.navigate,
        browserApi.back,
        browserApi.forward,
        browserApi.reload,
        browserApi.setBounds,
    ]) {
        vi.mocked(operation).mockResolvedValue(undefined as never);
    }
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    useToasts.setState({ toasts: [] });
});

/* The pane finds its agent through the store, the way the layout gives it to
   it, so the association has to exist before it renders. */
function renderPane(visible = true, painted = visible) {
    setState({
        browserPanes: { "pane-browser": "agent-one" },
        agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
    } as never);
    return render(<BrowserPaneHost paneId="pane-browser" visible={visible} painted={painted} onEmpty={onEmpty} />);
}

/** What the app's one reader of the strips would have put in the store. */
function announceStrip(strip: BrowserSnapshot) {
    return act(async () => {
        setState({ browserStrips: { "agent-one": strip } } as never);
    });
}

const onEmpty = vi.fn();

const placed = { x: 640, y: 96, width: 480, height: 321 };

/** Stands in for the stage telling the panes on it that it is travelling. */
function Stage({ moving }: { moving: boolean }) {
    useStageMotion(moving);
    return null;
}

describe("BrowserPaneHost", () => {
    it("opens the right-side browser when tabs appear and routes user tab actions", async () => {
        renderPane();

        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(screen.getByRole("region", { name: "codex browser" })).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: /New browser tab/ }));
        expect(browserApi.newTab).toHaveBeenCalledWith("agent-one");

        const address = screen.getByRole("textbox", { name: "Address and search" });
        fireEvent.change(address, { target: { value: "openai.com" } });
        fireEvent.submit(address.closest("form")!);
        expect(browserApi.navigate).toHaveBeenCalledWith("agent-one", "openai.com");
    });

    it("marks the tab the agent is working in with its colour and icon, beside the site's", async () => {
        renderPane();
        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(screen.queryByRole("img", { name: "codex is working in this tab" })).not.toBeInTheDocument();

        await announceStrip({
            tabs: [tab({ acting: true }), tab({ id: "tab-two", title: "Second", active: false })],
            activeTabId: "tab-one",
        });

        const working = screen.getByRole("img", { name: "codex is working in this tab" });
        expect(working.closest(".tab-wrap")).toHaveClass("acting");
        expect(screen.getByRole("tab", { name: /Second/ }).closest(".tab-wrap")).not.toHaveClass("acting");
    });

    /* A web app moving between its own screens never loads a document, so the
       address arrives on its own rather than with a page. */
    it("follows a page that changes its own address, and keeps out of the way of typing", async () => {
        renderPane();
        const address = await waitFor(() => screen.getByRole("textbox", { name: "Address and search" }));
        expect(address).toHaveValue("https://example.com");

        await announceStrip({ tabs: [tab({ url: "https://example.com/inbox" })], activeTabId: "tab-one" });
        expect(address).toHaveValue("https://example.com/inbox");

        fireEvent.focus(address);
        fireEvent.change(address, { target: { value: "openai.c" } });
        await announceStrip({ tabs: [tab({ url: "https://example.com/sent" })], activeTabId: "tab-one" });
        expect(address).toHaveValue("openai.c");

        fireEvent.blur(address);
        expect(address).toHaveValue("https://example.com/sent");
    });

    /* The page is a native view the window draws on its own; the pane only
       tells it where the page area is, in whole window pixels. */
    it("places the native page over the viewport and parks it when the pane hides", async () => {
        const { rerender } = renderPane();
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", placed));

        rerender(<BrowserPaneHost paneId="pane-browser" visible={false} painted={false} onEmpty={onEmpty} />);
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null));
    });

    it("re-places the page when its area moves and parks it on unmount", async () => {
        const { unmount } = renderPane();
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", placed));
        vi.mocked(browserApi.setBounds).mockClear();

        vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
            left: 700,
            top: 96,
            width: 420,
            height: 321,
            right: 0,
            bottom: 0,
            x: 700,
            y: 96,
            toJSON: () => ({}),
        });
        act(() => {
            for (const callback of resizeCallbacks) callback();
        });
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", { x: 700, y: 96, width: 420, height: 321 }));

        unmount();
        expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null);
    });

    it("steps the page aside while an app overlay is open", async () => {
        renderPane();
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", placed));

        let release = () => {};
        act(() => {
            release = occludeNativeViews();
        });
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null));

        act(() => release());
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", placed));
    });

    it("shows a themed blank surface instead of the page for a new tab", async () => {
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [tab({ url: "about:blank", title: "" })], activeTabId: "tab-one" });
        renderPane();
        await waitFor(() => expect(screen.getByRole("tab", { name: "New tab" })).toBeInTheDocument());
        expect(screen.getByLabelText("Blank browser page")).toBeInTheDocument();
        expect(screen.getByRole("textbox", { name: "Address and search" })).toHaveValue("");
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", null));
        expect(browserApi.setBounds).not.toHaveBeenCalledWith("agent-one", placed);
    });

    it("wears the site's own icon and falls back to a globe when it will not load", async () => {
        const icon = "data:image/png;base64,iVBORw0KGgo=";
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [tab({ favicon: icon })], activeTabId: "tab-one" });
        renderPane();
        const image = await waitFor(() => screen.getByRole("tab", { name: "Example" }).querySelector("img")!);
        expect(image).toHaveAttribute("src", icon);
        fireEvent.error(image);
        expect(screen.getByRole("tab", { name: "Example" }).querySelector("img")).toBeNull();
    });

    it("enables history buttons from what the page reports", async () => {
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [tab({ canGoBack: true, loading: true })], activeTabId: "tab-one" });
        renderPane();
        const back = await screen.findByRole("button", { name: "Back" });
        expect(back).toBeEnabled();
        expect(screen.getByRole("button", { name: "Forward" })).toBeDisabled();
        expect(back.closest("form")).toHaveClass("loading");
        fireEvent.click(back);
        expect(browserApi.back).toHaveBeenCalledWith("agent-one");
    });

    it("shows a tab the moment the strip reports one, and asks for the first read itself", async () => {
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [], activeTabId: null });
        renderPane();
        await waitFor(() => expect(browserApi.snapshot).toHaveBeenCalledWith("agent-one"));
        expect(screen.queryByRole("tab", { name: "Example" })).toBeNull();
        /* The pane is opened by the same click that asks for the tab, so it
           waits through the empty snapshot that arrives before the tab does. */
        expect(onEmpty).not.toHaveBeenCalled();

        await announceStrip(snapshot);

        expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument();
    });

    it("gives the pane up once the tab it held goes", async () => {
        renderPane();
        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(onEmpty).not.toHaveBeenCalled();

        await announceStrip({ tabs: [], activeTabId: null });

        expect(onEmpty).toHaveBeenCalled();
    });

    /* Tabs a restart saved are only worth a page once someone is looking at
       the pane, so nothing opens until it is on screen. */
    it("opens the tabs it was restored with, once, and shows the one that was in front", async () => {
        setState({ browserRestores: { "pane-browser": restored } } as never);
        vi.mocked(browserApi.newTab).mockImplementation(async (_agentId, url) => `tab-${url}`);
        const view = renderPane(false);

        expect(browserApi.newTab).not.toHaveBeenCalled();

        view.rerender(<BrowserPaneHost paneId="pane-browser" visible painted onEmpty={onEmpty} />);

        await waitFor(() => expect(browserApi.switchTab).toHaveBeenCalledWith("agent-one", "tab-https://second.test"));
        expect(vi.mocked(browserApi.newTab).mock.calls).toEqual([
            ["agent-one", "https://example.com"],
            ["agent-one", "https://second.test"],
        ]);
        expect(getState().browserRestores["pane-browser"]).toBeUndefined();
    });

    it("closes the pane when the tabs it was restored with cannot be opened", async () => {
        setState({ browserRestores: { "pane-browser": restored } } as never);
        vi.mocked(browserApi.newTab).mockRejectedValue(new Error("no window"));
        renderPane();

        await waitFor(() => expect(onEmpty).toHaveBeenCalled());
    });

    it("walks browser tabs with the arrow keys", async () => {
        vi.mocked(browserApi.snapshot).mockResolvedValue({
            tabs: [tab(), tab({ id: "tab-two", title: "Second", url: "https://second.test", active: false })],
            activeTabId: "tab-one",
        });
        renderPane();

        const first = await screen.findByRole("tab", { name: /Example/ });
        fireEvent.keyDown(first, { key: "ArrowRight" });

        await waitFor(() => expect(browserApi.switchTab).toHaveBeenCalledWith("agent-one", "tab-two"));
    });

    /* A swipe slides the screen this pane sits on, and the page has to go with
       it: the rect it is placed at changes every frame, with no scroll or
       resize to report it. */
    it("carries the page with its screen while the stage swipes", async () => {
        const frames: FrameRequestCallback[] = [];
        vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
        setState({
            browserPanes: { "pane-browser": "agent-one" },
            agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
        } as never);
        const swipe = (moving: boolean, visible: boolean, painted = true) => (
            <>
                <Stage moving={moving} />
                <BrowserPaneHost paneId="pane-browser" visible={visible} painted={painted} onEmpty={onEmpty} />
            </>
        );
        const { rerender } = render(swipe(false, true));
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", placed));

        // The screen being left is no longer the one the session is on, and its
        // page still has to show for as long as any of it is on the window.
        rerender(swipe(true, false));
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", placed));

        vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
            left: 240,
            top: 96,
            width: 480.4,
            height: 320.6,
            right: 0,
            bottom: 0,
            x: 240,
            y: 96,
            toJSON: () => ({}),
        });
        await act(async () => {
            frames.shift()?.(0);
        });
        expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", { ...placed, x: 240 });

        rerender(swipe(false, false));
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null));
    });

    /* Every session lays its screens over the same stage, and a screen that is
       not on it keeps its layout: the pane measures a rect over the window it
       may not draw in. The stage travels for all of them at once, so a swipe
       anywhere used to put those pages on screen for as long as it lasted. */
    it("leaves a screen that is not painting parked while the stage swipes", async () => {
        const frames: FrameRequestCallback[] = [];
        vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
        setState({
            browserPanes: { "pane-browser": "agent-one" },
            agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
            // The app reads every browsing agent's strip, looked at or not.
            browserStrips: { "agent-one": snapshot },
        } as never);
        render(
            <>
                <Stage moving />
                <BrowserPaneHost paneId="pane-browser" visible={false} painted={false} onEmpty={onEmpty} />
            </>,
        );

        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", null));
        expect(browserApi.setBounds).not.toHaveBeenCalledWith("agent-one", placed);
    });

    it("keeps a failed placement out of the way but reports it once", async () => {
        vi.mocked(browserApi.setBounds).mockRejectedValue(new Error("no window"));
        renderPane();
        await waitFor(() => expect(useToasts.getState().toasts.length).toBeGreaterThan(0));
    });
});
