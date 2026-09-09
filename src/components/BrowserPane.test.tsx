import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi, type BrowserFrame, type BrowserSnapshot } from "../api/browser";
import { AgentBrowserShell } from "./BrowserPane";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return {
        ...actual,
        browserApi: {
            snapshot: vi.fn(),
            startFrames: vi.fn(),
            newTab: vi.fn(),
            closeAgent: vi.fn(),
            switchTab: vi.fn(),
            closeTab: vi.fn(),
            navigate: vi.fn(),
            back: vi.fn(),
            forward: vi.fn(),
            reload: vi.fn(),
            pointer: vi.fn(),
            key: vi.fn(),
        },
    };
});

const snapshot: BrowserSnapshot = {
    tabs: [{ id: "tab-one", title: "Example", url: "https://example.com", active: true }],
    activeTabId: "tab-one",
};

beforeEach(() => {
    vi.stubGlobal(
        "ResizeObserver",
        class {
            observe() {}
            disconnect() {}
        },
    );
    vi.mocked(browserApi.snapshot).mockResolvedValue(snapshot);
    vi.mocked(browserApi.startFrames).mockImplementation(async (_agent, _target, _viewport, onFrame) => {
        onFrame({ data: "aGVsbG8=", width: 960, height: 640 });
        return vi.fn().mockResolvedValue(undefined);
    });
    for (const operation of [
        browserApi.newTab,
        browserApi.closeAgent,
        browserApi.switchTab,
        browserApi.closeTab,
        browserApi.navigate,
        browserApi.back,
        browserApi.forward,
        browserApi.reload,
        browserApi.pointer,
        browserApi.key,
    ]) {
        vi.mocked(operation).mockResolvedValue(undefined as never);
    }
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    vi.unstubAllGlobals();
});

describe("AgentBrowserShell", () => {
    it("opens the right-side browser when native tabs appear and routes user tab actions", async () => {
        const { container } = render(
            <AgentBrowserShell agentId="agent-one" agentType="codex" visible>
                <div>terminal</div>
            </AgentBrowserShell>,
        );

        expect(screen.getByText("terminal")).toBeInTheDocument();
        await waitFor(() => expect(screen.getByRole("tab", { name: "Example" })).toBeInTheDocument());
        expect(screen.getByRole("region", { name: "codex browser" })).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: /New browser tab/ }));
        expect(browserApi.newTab).toHaveBeenCalledWith("agent-one");

        const address = screen.getByRole("textbox", { name: "Address and search" });
        fireEvent.change(address, { target: { value: "openai.com" } });
        fireEvent.submit(address.closest("form")!);
        expect(browserApi.navigate).toHaveBeenCalledWith("agent-one", "openai.com");

        await waitFor(() => expect(container.querySelector(".browser-viewport > img[src]")).not.toBeNull());
        const viewport = container.querySelector(".browser-viewport")!;
        vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({ left: 0, top: 0, width: 480, height: 320 } as DOMRect);
        fireEvent.pointerMove(viewport, { clientX: 120, clientY: 80 });
        expect(browserApi.pointer).toHaveBeenCalledWith("agent-one", expect.objectContaining({ kind: "move", x: 240, y: 160 }));
    });

    it("stops hidden streams and ignores frames delivered after cleanup", async () => {
        const stop = vi.fn().mockResolvedValue(undefined);
        let receive: (frame: BrowserFrame) => void = () => {};
        vi.mocked(browserApi.startFrames).mockImplementation(async (_agent, _target, _viewport, onFrame) => {
            receive = onFrame;
            onFrame({ data: "first", width: 960, height: 640 });
            return stop;
        });
        const { container, rerender } = render(
            <AgentBrowserShell agentId="agent-one" agentType="codex" visible>
                <div>terminal</div>
            </AgentBrowserShell>,
        );
        await waitFor(() => expect(container.querySelector("img")?.getAttribute("src")).toContain("first"));
        rerender(
            <AgentBrowserShell agentId="agent-one" agentType="codex" visible={false}>
                <div>terminal</div>
            </AgentBrowserShell>,
        );
        await waitFor(() => expect(stop).toHaveBeenCalledOnce());
        act(() => receive({ data: "stale", width: 960, height: 640 }));
        expect(container.querySelector("img")?.getAttribute("src")).toBeNull();
    });

    it("stops a stream whose startup completes after the pane unmounts", async () => {
        const stop = vi.fn().mockResolvedValue(undefined);
        let finish: (stop: () => Promise<void>) => void = () => {};
        vi.mocked(browserApi.startFrames).mockImplementation(
            () =>
                new Promise((resolve) => {
                    finish = resolve;
                }),
        );
        const { unmount } = render(
            <AgentBrowserShell agentId="agent-one" agentType="codex" visible>
                <div>terminal</div>
            </AgentBrowserShell>,
        );
        await waitFor(() => expect(browserApi.startFrames).toHaveBeenCalledOnce());
        unmount();
        await act(async () => finish(stop));
        expect(stop).toHaveBeenCalledOnce();
    });

    it("replaces the frame stream when switching tabs without reusing the old image", async () => {
        const stop = vi.fn().mockResolvedValue(undefined);
        const callbacks: Array<(frame: BrowserFrame) => void> = [];
        vi.mocked(browserApi.startFrames).mockImplementation(async (_agent, target, _viewport, onFrame) => {
            callbacks.push(onFrame);
            onFrame({ data: target, width: 960, height: 640 });
            return stop;
        });
        const { container } = render(
            <AgentBrowserShell agentId="agent-one" agentType="codex" visible>
                <div>terminal</div>
            </AgentBrowserShell>,
        );
        await waitFor(() => expect(container.querySelector("img")?.getAttribute("src")).toContain("tab-one"));
        vi.mocked(browserApi.snapshot).mockResolvedValue({
            tabs: [{ id: "tab-two", title: "Second", url: "https://example.org", active: true }],
            activeTabId: "tab-two",
        });
        fireEvent.click(screen.getByRole("button", { name: /New browser tab/ }));
        await waitFor(() => expect(container.querySelector("img")?.getAttribute("src")).toContain("tab-two"));
        expect(stop).toHaveBeenCalledOnce();
        act(() => callbacks[0]({ data: "stale", width: 960, height: 640 }));
        expect(container.querySelector("img")?.getAttribute("src")).toContain("tab-two");
    });

    it("renders a themed native surface instead of Chromium's white blank frame", async () => {
        vi.mocked(browserApi.snapshot).mockResolvedValue({
            ...snapshot,
            tabs: [{ id: "blank-tab", title: "", url: "about:blank", active: true }],
            activeTabId: "blank-tab",
        });
        const { container } = render(
            <AgentBrowserShell agentId="agent-one" agentType="claude" visible>
                <div>terminal</div>
            </AgentBrowserShell>,
        );

        await waitFor(() => expect(screen.getByLabelText("Blank browser page")).toBeInTheDocument());
        expect(container.querySelector(".browser-viewport > img[src]")).toBeNull();
        const viewport = container.querySelector(".browser-viewport")!;
        fireEvent.pointerMove(viewport, { clientX: 12, clientY: 18 });
        fireEvent.pointerDown(viewport, { clientX: 12, clientY: 18 });
        fireEvent.wheel(viewport, { deltaY: 100 });
        expect(browserApi.pointer).not.toHaveBeenCalled();
    });

    it("captures pointer drags and releases Chromium on cancellation", async () => {
        const { container } = render(
            <AgentBrowserShell agentId="agent-one" agentType="codex" visible>
                <div>terminal</div>
            </AgentBrowserShell>,
        );
        await waitFor(() => expect(container.querySelector(".browser-viewport > img[src]")).not.toBeNull());
        const viewport = container.querySelector<HTMLElement>(".browser-viewport")!;
        const capture = vi.fn();
        const release = vi.fn();
        Object.defineProperties(viewport, {
            setPointerCapture: { value: capture },
            hasPointerCapture: { value: () => true },
            releasePointerCapture: { value: release },
        });

        fireEvent.pointerDown(viewport, { pointerId: 7, clientX: 12, clientY: 18 });
        fireEvent.pointerCancel(viewport, { pointerId: 7, clientX: 20, clientY: 24 });

        expect(capture).toHaveBeenCalledWith(7);
        expect(release).toHaveBeenCalledWith(7);
        expect(browserApi.pointer).toHaveBeenCalledWith("agent-one", expect.objectContaining({ kind: "down" }));
        expect(browserApi.pointer).toHaveBeenCalledWith("agent-one", expect.objectContaining({ kind: "up" }));
    });
});
