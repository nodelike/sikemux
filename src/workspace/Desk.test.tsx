import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi, takeKeyboardFromPages, type BrowserSnapshot, type BrowserTab } from "../api/browser";
import { occludeNativeViews, setNativeViewHoles, useStageMotion } from "../state/nativeViews";
import { useToasts } from "../state/toast";
import { getState, setState } from "../state/store";
import { deskEditorId } from "../state/desks";
import { taskPtyBindings } from "../tasks/nativeRuntime";
import type { Session, Window as WindowT } from "../state/types";
import * as cmd from "../state/commands";
import { DeskHost } from "./Desk";

vi.mock("../editor/EditorPane", () => ({
    EditorPane: ({ paneId, visible }: { paneId: string; visible: boolean }) => (
        <div data-testid="desk-editor" data-pane={paneId} data-visible={String(visible)} />
    ),
}));

vi.mock("../terminal/TerminalPane", () => ({
    TerminalPane: ({ context, visible }: { context: { paneId: string }; visible: boolean }) => (
        <div data-testid="desk-terminal" data-pane={context.paneId} data-visible={String(visible)} />
    ),
}));

vi.mock("../sim/SimulatorPane", () => ({
    SimulatorPane: ({ simulator, visible }: { simulator: { id: string }; visible: boolean }) => (
        <div data-testid="desk-simulator" data-simulator={simulator.id} data-visible={String(visible)} />
    ),
}));

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return {
        ...actual,
        takeKeyboardFromPages: vi.fn().mockResolvedValue(undefined),
        browserApi: {
            snapshot: vi.fn(),
            newTab: vi.fn(),
            closeAgent: vi.fn(),
            switchTab: vi.fn(),
            closeTab: vi.fn(),
            navigate: vi.fn(),
            suggest: vi.fn(),
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

/** A desk's worth of saved pages, as hydration would hand them over. */
const restored = {
    agentId: "agent-one",
    tabs: [
        { url: "https://example.com", title: "Example" },
        { url: "https://second.test", title: "Second" },
    ],
    activeIndex: 1,
    files: [],
};

const session = { id: "project", name: "project", kind: "project", cwd: "/repo", activeWindowId: "window" } as Session;
const win = { id: "window", name: "1", role: "agent", activePaneId: "agent-one" } as WindowT;

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
    setState({ browserStrips: {}, deskRestores: {}, desks: {}, editorViews: {}, deskAddressOpen: null } as never);
    vi.mocked(browserApi.snapshot).mockResolvedValue(snapshot);
    vi.mocked(browserApi.subscribeTabs).mockResolvedValue(vi.fn());
    vi.mocked(browserApi.suggest).mockResolvedValue({ completion: null, pages: [], searches: false, searchUrl: "" });
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

function Host({ visible, painted = visible }: { visible: boolean; painted?: boolean }) {
    return <DeskHost paneId="pane-desk" session={session} win={win} active={visible} visible={visible} painted={painted} onEmpty={onEmpty} />;
}

/* The pane finds its agent through the store, the way the layout gives it to
   it, so the association has to exist before it renders. */
function renderPane(visible = true, painted = visible) {
    setState({
        deskPanes: { "pane-desk": "agent-one" },
        agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
    } as never);
    return render(<Host visible={visible} painted={painted} />);
}

/** What the app's one reader of the strips would have put in the store. */
function announceStrip(strip: BrowserSnapshot) {
    return act(async () => {
        setState({ browserStrips: { "agent-one": strip } } as never);
    });
}

const onEmpty = vi.fn();

const placed = { x: 640, y: 96, width: 480, height: 321, clipLeft: 0, clipRight: 0, holes: [] };

/** Stands in for the stage telling the panes on it that it is travelling. */
function Stage({ moving }: { moving: boolean }) {
    useStageMotion(moving);
    return null;
}

describe("DeskHost", () => {
    it("shows the agent's pages when tabs appear and routes user tab actions", async () => {
        renderPane();

        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(screen.getByRole("region", { name: "codex desk" })).toBeInTheDocument();

        act(() => {
            fireEvent.click(screen.getByRole("button", { name: /New browser tab/ }));
        });
        expect(browserApi.newTab).toHaveBeenCalledWith("agent-one");

        const panel = await screen.findByRole("dialog", { name: "Open address" });
        const address = within(panel).getByRole("textbox", { name: "Address and search" });
        await waitFor(() => expect(address).toHaveFocus());
        fireEvent.change(address, { target: { value: "openai.com" } });
        fireEvent.keyDown(address, { key: "Enter" });
        expect(browserApi.navigate).toHaveBeenCalledWith("agent-one", "openai.com");
    });

    /* A page is a webview of its own and keeps the keyboard it had, so the app's
       webview has to take it back before the field can have it. */
    it("opens the address over the middle of the page on the address shortcut, with the keyboard taken from the page", async () => {
        renderPane();
        const agentPane = { type: "pane", id: "agent-one", cwd: "/repo", kind: "agent", title: "codex" };
        const deskPane = { type: "pane", id: "pane-desk", cwd: "/repo", kind: "desk", title: "desk" };
        setState({
            sessions: { project: session },
            activeSessionId: "project",
            windows: { window: { ...win, root: { type: "split", id: "split", dir: "row", children: [agentPane, deskPane], sizes: [50, 50] } } },
        } as never);
        await announceStrip(snapshot);
        const toolbarField = screen.getByRole("textbox", { name: "Address and search" });

        act(() => {
            expect(cmd.focusBrowserAddress()).toBe(true);
        });

        const panel = await screen.findByRole("dialog", { name: "Open address" });
        const field = within(panel).getByRole("textbox", { name: "Address and search" });
        await waitFor(() => expect(field).toHaveFocus());
        expect(takeKeyboardFromPages).toHaveBeenCalled();
        expect(browserApi.newTab).not.toHaveBeenCalled();
        expect(toolbarField).toHaveValue("");
        expect(toolbarField).not.toHaveAttribute("placeholder");
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", expect.objectContaining({ dim: 0.2 })));

        fireEvent.keyDown(field, { key: "Escape" });
        await waitFor(() => expect(screen.queryByRole("dialog", { name: "Open address" })).not.toBeInTheDocument());
        expect(getState().deskAddressOpen).toBeNull();
        expect(toolbarField).toHaveValue("https://example.com");
        await waitFor(() => expect(vi.mocked(browserApi.setBounds).mock.lastCall?.[1]).not.toHaveProperty("dim"));
    });

    it("lets go of the address on Escape and drops what was typed", async () => {
        renderPane();
        await announceStrip(snapshot);
        const address = screen.getByRole("textbox", { name: "Address and search" });
        address.focus();
        fireEvent.change(address, { target: { value: "half-typ" } });

        fireEvent.keyDown(address, { key: "Escape" });

        expect(address).not.toHaveFocus();
        expect(address).toHaveValue("https://example.com");
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
        expect(working.closest(".tab-tail")?.querySelector(".tab-x")).not.toBeNull();
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

        rerender(<Host visible={false} />);
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null));
    });

    /* A toast is too brief to send the page away for, so the page leaves a hole
       where it sits, in the page's own coordinates. */
    it("cuts a hole in the page where a toast sits over it, and only there", async () => {
        renderPane();
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", placed));

        const toasts = {};
        act(() =>
            setNativeViewHoles(toasts, [
                { x: 600, y: 380, width: 200, height: 34, radius: 13 },
                { x: 10, y: 380, width: 200, height: 34, radius: 13 },
            ]),
        );
        await waitFor(() =>
            expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", {
                ...placed,
                holes: [{ x: -40, y: 284, width: 200, height: 34, radius: 13 }],
            }),
        );

        act(() => setNativeViewHoles(toasts, []));
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", placed));
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
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", { ...placed, x: 700, width: 420 }));

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
        expect(back.closest(".browser-toolbar")).toHaveClass("loading");
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

    it("gives the pane up once the page it held goes", async () => {
        renderPane();
        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(onEmpty).not.toHaveBeenCalled();

        await announceStrip({ tabs: [], activeTabId: null });

        expect(onEmpty).toHaveBeenCalled();
    });

    /* Tabs a restart saved are only worth a page once someone is looking at
       the pane, so nothing opens until it is on screen. */
    it("opens the tabs it was restored with, once, and shows the one that was in front", async () => {
        setState({ deskRestores: { "pane-desk": restored } } as never);
        vi.mocked(browserApi.newTab).mockImplementation(async (_agentId, url) => `tab-${url}`);
        const view = renderPane(false);

        expect(browserApi.newTab).not.toHaveBeenCalled();

        view.rerender(<Host visible />);

        await waitFor(() => expect(browserApi.switchTab).toHaveBeenCalledWith("agent-one", "tab-https://second.test"));
        expect(vi.mocked(browserApi.newTab).mock.calls).toEqual([
            ["agent-one", "https://example.com"],
            ["agent-one", "https://second.test"],
        ]);
        expect(getState().deskRestores["pane-desk"]).toBeUndefined();
    });

    it("closes the pane when the pages it was restored with cannot be opened", async () => {
        setState({ deskRestores: { "pane-desk": restored } } as never);
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [], activeTabId: null });
        vi.mocked(browserApi.newTab).mockRejectedValue(new Error("no window"));
        renderPane();

        await waitFor(() => expect(onEmpty).toHaveBeenCalled());
    });

    it("walks desk tabs with the arrow keys", async () => {
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
            deskPanes: { "pane-desk": "agent-one" },
            agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
        } as never);
        const swipe = (moving: boolean, visible: boolean, painted = true) => (
            <>
                <Stage moving={moving} />
                <Host visible={visible} painted={painted} />
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
            for (const frame of frames.splice(0)) frame(0);
        });
        expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", { ...placed, x: 240 });

        rerender(swipe(false, false));
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null));
    });

    /* A native view is not cut off by the stage the way the DOM around it is,
       so the part of the page slid past the stage's edge is cropped by hand. */
    it("crops the part of a travelling page that has left the stage", async () => {
        const frames: FrameRequestCallback[] = [];
        vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
        const rect = (left: number, width: number) => ({
            left,
            top: 96,
            width,
            height: 320.6,
            right: left + width,
            bottom: 0,
            x: left,
            y: 96,
            toJSON: () => ({}),
        });
        let pageLeft = 640;
        vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
            return this.classList.contains("window-area") ? rect(300, 900) : rect(pageLeft, 480.4);
        });
        setState({
            deskPanes: { "pane-desk": "agent-one" },
            agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
            browserStrips: { "agent-one": snapshot },
        } as never);
        render(
            <div className="window-area">
                <Stage moving />
                <Host visible={false} painted />
            </div>,
        );
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", placed));

        pageLeft = 200;
        await act(async () => {
            for (const frame of frames.splice(0)) frame(0);
        });
        expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", { ...placed, x: 200, clipLeft: 100 });

        pageLeft = 1000;
        await act(async () => {
            for (const frame of frames.splice(0)) frame(0);
        });
        expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", { ...placed, x: 1000, clipRight: 280 });

        pageLeft = -300;
        await act(async () => {
            for (const frame of frames.splice(0)) frame(0);
        });
        expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null);
    });

    /* Every session lays its screens over the same stage, and a screen that is
       not on it keeps its layout: the pane measures a rect over the window it
       may not draw in. The stage travels for all of them at once, so a swipe
       anywhere used to put those pages on screen for as long as it lasted. */
    it("leaves a screen that is not painting parked while the stage swipes", async () => {
        const frames: FrameRequestCallback[] = [];
        vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
        setState({
            deskPanes: { "pane-desk": "agent-one" },
            agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
            // The app reads every browsing agent's strip, looked at or not.
            browserStrips: { "agent-one": snapshot },
        } as never);
        render(
            <>
                <Stage moving />
                <Host visible={false} />
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

    it("keeps pages, files and terminals in one strip, in the order they arrived", async () => {
        taskPtyBindings.bind("term-web", { executionId: "run-1", terminalKey: "task-web", ptyId: 7 } as never);
        setState({
            desks: {
                "agent-one": {
                    order: ["file:/repo/src/a.ts", "browser:tab-one", "terminal:term-web"],
                    active: "file:/repo/src/a.ts",
                    terminals: [{ id: "term-web", terminalKey: "task-web", label: "Web", cwd: "/repo" }],
                    simulators: [],
                    reveal: null,
                },
            },
            editorViews: { [deskEditorId("agent-one")]: { openTabs: ["/repo/src/a.ts"], activePath: "/repo/src/a.ts" } },
        } as never);
        renderPane();

        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(screen.getAllByRole("tab").map((tab) => tab.textContent?.replace(/[^\x20-\x7e]/g, ""))).toEqual(["a.ts", "Example", "Web"]);
        expect(screen.getByRole("tab", { name: /a\.ts/ })).toHaveAttribute("aria-selected", "true");
        expect(await screen.findByTestId("desk-editor")).toHaveAttribute("data-visible", "true");
        expect(screen.getByTestId("desk-terminal")).toHaveAttribute("data-pane", "term-web");
        expect(screen.getByTestId("desk-terminal")).toHaveAttribute("data-visible", "false");
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", null));

        fireEvent.click(screen.getByRole("tab", { name: "Web" }));

        expect(getState().desks["agent-one"].active).toBe("terminal:term-web");
        expect(screen.getByTestId("desk-terminal")).toHaveAttribute("data-visible", "true");
        expect(screen.getByTestId("desk-editor")).toHaveAttribute("data-visible", "false");

        fireEvent.click(screen.getByRole("tab", { name: "Example" }));

        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", placed));
        taskPtyBindings.release("term-web");
    });

    it("names the simulator's tab after its device and shows its pane only while the tab is chosen", async () => {
        setState({
            desks: {
                "agent-one": {
                    order: ["file:/repo/src/a.ts", "simulator:sim-1"],
                    active: "file:/repo/src/a.ts",
                    terminals: [],
                    simulators: [{ id: "sim-1", udid: "UDID-1", deviceName: "iPhone 17" }],
                    reveal: null,
                },
            },
            editorViews: { [deskEditorId("agent-one")]: { openTabs: ["/repo/src/a.ts"], activePath: "/repo/src/a.ts" } },
        } as never);
        renderPane();

        const tab = await screen.findByRole("tab", { name: /iPhone 17/ });
        expect(await screen.findByTestId("desk-simulator")).toHaveAttribute("data-visible", "false");

        fireEvent.click(tab);

        expect(getState().desks["agent-one"].active).toBe("simulator:sim-1");
        expect(screen.getByTestId("desk-simulator")).toHaveAttribute("data-visible", "true");
    });

    it("stays open for a file that is still on its way to the editor", async () => {
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [], activeTabId: null });
        setState({
            desks: {
                "agent-one": {
                    order: ["file:/repo/b.ts"],
                    active: "file:/repo/b.ts",
                    terminals: [],
                    simulators: [],
                    reveal: { path: "/repo/b.ts", seq: 1 },
                },
            },
        } as never);
        renderPane();

        expect(await screen.findByTestId("desk-editor")).toHaveAttribute("data-pane", deskEditorId("agent-one"));
        expect(onEmpty).not.toHaveBeenCalled();
    });

    it("closes a pane restored for an agent that is no longer there", () => {
        setState({ deskPanes: {}, agents: {} } as never);
        const view = render(<Host visible />);

        expect(onEmpty).toHaveBeenCalled();
        expect(view.container).toBeEmptyDOMElement();
        expect(browserApi.snapshot).not.toHaveBeenCalled();
    });

    it("names an untitled tab by its address and marks one still loading", async () => {
        renderPane();
        await screen.findByRole("tab", { name: "Example" });
        await announceStrip({
            tabs: [tab({ title: "", url: "https://untitled.test/page" }), tab({ id: "tab-two", title: "Busy", active: false, loading: true })],
            activeTabId: "tab-one",
        });

        expect(screen.getByRole("tab", { name: "https://untitled.test/page" })).toBeInTheDocument();
        expect(screen.getByRole("tab", { name: /Busy/ }).closest(".tab-wrap")?.querySelector('[aria-label="Loading"]')).not.toBeNull();
        expect(screen.getByRole("textbox", { name: "Address and search" })).toHaveValue("https://untitled.test/page");
    });

    it("closes the page whose tab is closed and reads the strip again", async () => {
        renderPane();
        const example = await screen.findByRole("tab", { name: "Example" });
        vi.mocked(browserApi.snapshot).mockClear();

        fireEvent.click(example.closest(".tab-wrap")!.querySelector(".tab-x")!);

        await waitFor(() => expect(browserApi.closeTab).toHaveBeenCalledWith("agent-one", "tab-one"));
        await waitFor(() => expect(browserApi.snapshot).toHaveBeenCalledWith("agent-one"));
    });

    it("sends forward and reload to the page and reads the strip again after each", async () => {
        vi.mocked(browserApi.snapshot).mockResolvedValue({ tabs: [tab({ canGoForward: true })], activeTabId: "tab-one" });
        renderPane();
        const forward = await screen.findByRole("button", { name: "Forward" });
        await waitFor(() => expect(forward).toBeEnabled());
        vi.mocked(browserApi.snapshot).mockClear();

        fireEvent.click(forward);
        fireEvent.click(screen.getByRole("button", { name: "Reload" }));

        expect(browserApi.forward).toHaveBeenCalledWith("agent-one");
        expect(browserApi.reload).toHaveBeenCalledWith("agent-one");
        await waitFor(() => expect(browserApi.snapshot).toHaveBeenCalledTimes(2));
    });

    it("reports a page action that fails", async () => {
        vi.mocked(browserApi.reload).mockRejectedValue(new Error("gone"));
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: "Reload" }));

        await waitFor(() => expect(useToasts.getState().toasts.length).toBeGreaterThan(0));
    });

    it("drops what was typed in the address bar once it loses focus", async () => {
        renderPane();
        const address = await screen.findByRole("textbox", { name: "Address and search" });
        await waitFor(() => expect(address).toHaveValue("https://example.com"));

        fireEvent.focus(address);
        fireEvent.change(address, { target: { value: "half typ" } });
        expect(address).toHaveValue("half typ");
        fireEvent.blur(address);

        expect(address).toHaveValue("https://example.com");
    });

    it("opens nothing for a restore that saved no pages", async () => {
        setState({ deskRestores: { "pane-desk": { ...restored, tabs: [] } } } as never);
        renderPane();

        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(browserApi.newTab).not.toHaveBeenCalled();
    });

    it("keeps the pane when restored pages fail to open but it already holds a page", async () => {
        setState({ deskRestores: { "pane-desk": { ...restored, activeIndex: 5 } } } as never);
        vi.mocked(browserApi.newTab).mockRejectedValue(new Error("no window"));
        renderPane();
        await announceStrip(snapshot);

        await waitFor(() => expect(useToasts.getState().toasts.length).toBeGreaterThan(0));
        expect(onEmpty).not.toHaveBeenCalled();
    });

    it("brings no restored tab to the front when the saved front tab did not open", async () => {
        setState({ deskRestores: { "pane-desk": { ...restored, activeIndex: 5 } } } as never);
        vi.mocked(browserApi.newTab).mockImplementation(async (_agentId, url) => `tab-${url}`);
        renderPane();

        await waitFor(() => expect(browserApi.newTab).toHaveBeenCalledTimes(2));
        await waitFor(() => expect(browserApi.snapshot).toHaveBeenCalledTimes(2));
        expect(browserApi.switchTab).not.toHaveBeenCalled();
    });

    it("re-places the page when a scrolling ancestor scrolls", async () => {
        setState({
            deskPanes: { "pane-desk": "agent-one" },
            agents: { "agent-one": { id: "agent-one", type: "codex", title: "codex", launchState: "live" } },
        } as never);
        render(
            <div data-testid="scroller" style={{ overflowY: "auto" }}>
                <Host visible />
            </div>,
        );
        await waitFor(() => expect(browserApi.setBounds).toHaveBeenCalledWith("agent-one", placed));

        vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
            left: 640,
            top: 40,
            width: 480.4,
            height: 320.6,
            right: 0,
            bottom: 0,
            x: 640,
            y: 40,
            toJSON: () => ({}),
        });
        fireEvent.scroll(screen.getByTestId("scroller"));

        await waitFor(() => expect(browserApi.setBounds).toHaveBeenLastCalledWith("agent-one", { ...placed, y: 40 }));
    });
});
