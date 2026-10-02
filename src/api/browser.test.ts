import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi, type BrowserDownload, type BrowserShortcut } from "./browser";
import { MemoryIpcTransport, installIpcTransportForTests, resetIpcTransportForTests } from "./transport";

let transport: MemoryIpcTransport;

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
});

afterEach(() => {
    resetIpcTransportForTests();
});

describe("browserApi", () => {
    it("opens a blank tab when no address is given, and the one asked for otherwise", async () => {
        const handler = vi.fn((_args: unknown) => Promise.resolve("tab-1"));
        transport.register("browser_new_tab", handler);

        expect(await browserApi.newTab("agent-1")).toBe("tab-1");
        await browserApi.newTab("agent-1", "https://example.com");

        expect(handler.mock.calls.map(([args]) => args)).toEqual([
            { agentId: "agent-1", url: null },
            { agentId: "agent-1", url: "https://example.com" },
        ]);
    });

    it("gives up on a snapshot whose caller has stopped waiting", async () => {
        const snapshot = { tabs: [], activeTabId: null };
        transport.register("browser_snapshot", () => Promise.resolve(snapshot));
        const controller = new AbortController();
        controller.abort();

        expect(await browserApi.snapshot("agent-1")).toEqual(snapshot);
        await expect(browserApi.snapshot("agent-1", controller.signal)).rejects.toThrow();
    });

    it("hands each listener the payload its event carries, until the caller stops listening", async () => {
        const downloads: BrowserDownload[] = [];
        const shortcuts: BrowserShortcut[] = [];
        const acting: string[] = [];
        const tabs = vi.fn();
        const controller = new AbortController();
        await Promise.all([
            browserApi.subscribeDownloads((download) => downloads.push(download), controller.signal),
            browserApi.subscribeShortcuts((shortcut) => shortcuts.push(shortcut), controller.signal),
            browserApi.subscribeActing((agentId) => acting.push(agentId), controller.signal),
            browserApi.subscribeTabs(tabs, controller.signal),
        ]);
        const download: BrowserDownload = { agentId: "a", tabId: "t", url: "https://x.test/f", path: "/tmp/f", state: "started" };
        const shortcut: BrowserShortcut = { agentId: "a", tabId: "t", key: "l", code: "KeyL", shift: false, alt: false };

        transport.emit("browser-download", download);
        transport.emit("browser-shortcut", shortcut);
        transport.emit("browser-agent-acting", "agent-1");
        transport.emit("browser-tabs-changed", null);
        controller.abort();
        transport.emit("browser-agent-acting", "agent-2");
        transport.emit("browser-tabs-changed", null);

        expect(downloads).toEqual([download]);
        expect(shortcuts).toEqual([shortcut]);
        expect(acting).toEqual(["agent-1"]);
        expect(tabs).toHaveBeenCalledOnce();
    });
});
