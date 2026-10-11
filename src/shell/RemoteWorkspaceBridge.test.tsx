import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteStatus } from "../api/remote";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import * as cmd from "../state/commands";
import { resetResourcesForTests } from "../state/resources";
import { getState, setState } from "../state/store";
import { PUBLISH_DELAY_MS, RECENT_LIMIT, RECENT_REFRESH_MS, RemoteWorkspaceBridge } from "./RemoteWorkspaceBridge";

const initial = getState();

function status(enabled: boolean): RemoteStatus {
    return {
        enabled,
        coreId: "core",
        addresses: [],
        devices: [],
        connected: [],
        pending: [],
        owner: null,
        account: null,
        updateRequired: null,
        notifications: [],
    };
}

let transport: MemoryIpcTransport;
let publish: ReturnType<typeof vi.fn<(args: unknown) => void>>;

beforeEach(() => {
    vi.useFakeTimers();
    setState(initial, true);
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    publish = vi.fn<(args: unknown) => void>();
    transport.register("remote_publish_workspace", publish);
});

afterEach(() => {
    cleanup();
    resetResourcesForTests();
    resetIpcTransportForTests();
    vi.useRealTimers();
});

async function settle() {
    await act(async () => {
        await vi.advanceTimersByTimeAsync(PUBLISH_DELAY_MS + 50);
    });
}

describe("RemoteWorkspaceBridge", () => {
    it("publishes the workspace with remote access off, and again when a project opens", async () => {
        transport.register("remote_status", () => status(false));
        render(<RemoteWorkspaceBridge />);
        await settle();
        expect(publish).toHaveBeenCalledTimes(1);
        const before = publish.mock.calls[0][0] as { projects: { path: string }[] };

        act(() => {
            cmd.createProjectSession("/Users/me/new-project");
        });
        await settle();
        expect(publish).toHaveBeenCalledTimes(2);
        const after = publish.mock.calls[1][0] as { projects: { path: string }[]; launchers: { id: string }[] };
        expect(after.projects.length).toBe(before.projects.length + 1);
        expect(after.projects.map((project) => project.path)).toContain("/Users/me/new-project");
        expect(after.launchers.map((launcher) => launcher.id)).toContain("opencode");
    });

    it("publishes what the person named their agents and which one is on screen", async () => {
        transport.register("remote_status", () => status(false));
        const agents = vi.fn<(args: unknown) => void>();
        const onScreen = vi.fn<(args: unknown) => void>();
        transport.register("remote_publish_agents", agents);
        transport.register("remote_publish_on_screen", onScreen);
        render(<RemoteWorkspaceBridge />);
        await settle();
        act(() => {
            cmd.createProjectSession("/Users/me/notch");
            cmd.addAgent("claude", undefined, "Fix the login flake");
        });
        await settle();
        const agentId = Object.values(getState().agents).find((agent) => agent.title === "Fix the login flake")?.id ?? "";
        const titles = (agents.mock.lastCall?.[0] as { titles: Record<string, string> }).titles;
        expect(titles[agentId]).toBe("Fix the login flake");
        expect(onScreen.mock.lastCall?.[0]).toEqual({ agentIds: [agentId] });
    });

    it("publishes no agent on screen while the window is behind another app", async () => {
        transport.register("remote_status", () => status(false));
        const onScreen = vi.fn<(args: unknown) => void>();
        transport.register("remote_publish_on_screen", onScreen);
        render(<RemoteWorkspaceBridge />);
        act(() => {
            cmd.createProjectSession("/Users/me/notch");
            cmd.addAgent("claude", undefined, "Watch the build");
        });
        await settle();
        expect(onScreen.mock.lastCall?.[0]).not.toEqual({ agentIds: [] });
        act(() => {
            window.dispatchEvent(new Event("blur"));
        });
        await settle();
        expect(onScreen.mock.lastCall?.[0]).toEqual({ agentIds: [] });
        act(() => {
            window.dispatchEvent(new Event("focus"));
        });
        await settle();
        expect(onScreen.mock.lastCall?.[0]).not.toEqual({ agentIds: [] });
    });

    it("publishes the newest saved chats while remote access is on, leaving open ones out, and refreshes them", async () => {
        transport.register("remote_status", () => status(true));
        transport.register("available_agents", () => [
            { type: "claude", label: "Claude", command: "/bin/claude", configPath: "/Users/me/.claude" },
            { type: "pi", label: "Pi", command: "/bin/pi" },
        ]);
        const scans = vi.fn<(args: unknown) => void>();
        transport.register("agent_recent_sessions", (args) => {
            scans(args);
            return {
                sessions: [{ agent: "claude", id: "saved-1", title: "Fix the login flake", mtime: 1_700_000_000, project: "/Users/me/notch" }],
                next: null,
            };
        });
        const recent = vi.fn<(args: unknown) => void>();
        transport.register("remote_publish_recent", recent);
        render(<RemoteWorkspaceBridge />);
        act(() => {
            cmd.createProjectSession("/Users/me/notch");
            cmd.addAgent("claude", "open-1", "Already open");
        });
        await settle();
        await settle();
        const { request } = scans.mock.lastCall?.[0] as {
            request: { providers: { agent: string }[]; projects: string[]; limit: number; exclude: { agent: string; id: string }[] };
        };
        expect(request.providers.map((provider) => provider.agent)).toEqual(["claude"]);
        expect(request.projects).toContain("/Users/me/notch");
        expect(request.limit).toBe(RECENT_LIMIT);
        expect(request.exclude).toContainEqual({ agent: "claude", id: "open-1" });
        expect(recent.mock.lastCall?.[0]).toEqual({
            chats: [
                {
                    launcher: "claude:builtin-claude",
                    provider: "claude",
                    sessionId: "saved-1",
                    title: "Fix the login flake",
                    cwd: "/Users/me/notch",
                    activeAt: 1_700_000_000_000,
                },
            ],
        });

        const published = recent.mock.calls.length;
        const scanned = scans.mock.calls.length;
        await act(async () => {
            await vi.advanceTimersByTimeAsync(RECENT_REFRESH_MS);
        });
        expect(scans.mock.calls.length).toBeGreaterThan(scanned);
        expect(recent.mock.calls.length).toBe(published);
    });
});
