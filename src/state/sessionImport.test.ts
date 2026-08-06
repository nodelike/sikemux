import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { importSessionFromClipboard } from "./commands";
import { getState, setState } from "./store";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
});

describe("session clipboard import", () => {
    it("does not mutate state when any row in the bundle is invalid", async () => {
        const before = getState();
        const raw = JSON.stringify({
            format: "sikemux-session",
            version: 1,
            session: { name: "demo", cwd: "/work", kind: "project" },
            windows: [
                {
                    id: "window-1",
                    name: "1",
                    role: "term",
                    root: { type: "pane", id: "pane-1", cwd: "/work", kind: "terminal", title: "shell" },
                    activePaneId: "pane-1",
                },
            ],
            agents: [
                { type: "codex", title: "one", resumeId: "duplicate" },
                { type: "codex", title: "two", resumeId: "duplicate" },
            ],
        });
        Object.defineProperty(navigator, "clipboard", { configurable: true, value: { readText: vi.fn().mockResolvedValue(raw) } });

        await expect(importSessionFromClipboard()).rejects.toThrow("duplicate agent session claim");
        expect(getState()).toBe(before);
    });
});
