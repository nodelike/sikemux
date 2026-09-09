import { Channel } from "@tauri-apps/api/core";
import { invokeCommand as invoke } from "./invoke";

export interface BrowserTab {
    id: string;
    title: string;
    url: string;
    active: boolean;
}

export interface BrowserSnapshot {
    tabs: BrowserTab[];
    activeTabId: string | null;
}

export interface BrowserFrame {
    data: string;
    width: number;
    height: number;
}

export interface BrowserViewport {
    width: number;
    height: number;
}

export interface BrowserPointerInput {
    kind: "move" | "down" | "up" | "wheel";
    x: number;
    y: number;
    button?: "none" | "left" | "middle" | "right";
    deltaX?: number;
    deltaY?: number;
}

export interface BrowserKeyInput {
    kind: "down" | "up" | "text";
    key: string;
    code: string;
    text?: string;
    modifiers?: number;
}

export const browserApi = {
    snapshot: (agentId: string, signal?: AbortSignal) => invoke<BrowserSnapshot>("browser_snapshot", { agentId }, signal ? { signal } : undefined),
    startFrames: async (agentId: string, targetId: string, viewport: BrowserViewport, onFrame: (frame: BrowserFrame) => void) => {
        const channel = new Channel<BrowserFrame>();
        channel.onmessage = onFrame;
        const streamId = await invoke<number>("browser_start_frames", { agentId, targetId, viewport, onFrame: channel });
        return () => invoke<void>("browser_stop_frames", { agentId, streamId });
    },
    newTab: (agentId: string, url?: string) => invoke<string>("browser_new_tab", { agentId, url: url ?? null }),
    closeAgent: (agentId: string) => invoke<void>("browser_close_agent", { agentId }),
    switchTab: (agentId: string, targetId: string) => invoke<void>("browser_switch_tab", { agentId, targetId }),
    closeTab: (agentId: string, targetId: string) => invoke<void>("browser_close_tab", { agentId, targetId }),
    navigate: (agentId: string, url: string) => invoke<void>("browser_navigate", { agentId, url }),
    back: (agentId: string) => invoke<void>("browser_back", { agentId }),
    forward: (agentId: string) => invoke<void>("browser_forward", { agentId }),
    reload: (agentId: string) => invoke<void>("browser_reload", { agentId }),
    pointer: (agentId: string, input: BrowserPointerInput) => invoke<void>("browser_pointer", { agentId, input }),
    key: (agentId: string, input: BrowserKeyInput) => invoke<void>("browser_key", { agentId, input }),
};
