import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
    available: vi.fn(),
    sessions: vi.fn(),
    usage: vi.fn(),
}));

vi.mock("../api/agents", () => ({
    agentApi: { available: mocks.available, sessions: mocks.sessions, usage: mocks.usage },
}));

// jsdom has no ResizeObserver; the rail uses one to keep filling its list.
vi.stubGlobal(
    "ResizeObserver",
    class {
        observe() {}
        disconnect() {}
    },
);

import { invalidate } from "../state/resources";
import { getState, setState } from "../state/store";
import { AgentRailBody } from "./AgentRail";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    setState({
        sessions: {
            "sess-project": {
                id: "sess-project",
                name: "sikemux",
                kind: "project" as const,
                cwd: "/code/sikemux",
                deploy: null,
                pinned: false,
                activeWindowId: "win-project",
                activeAgentId: null,
                view: "agent" as const,
            },
        },
        sessionOrder: ["sess-project"],
        activeSessionId: "sess-project",
        agents: {},
        agentsBySession: { "sess-project": [] },
    });
    mocks.available.mockResolvedValue([{ type: "codex", label: "Codex", command: "codex", defaultModel: "gpt-5.6-sol", defaultEffort: "high" }]);
    mocks.sessions.mockResolvedValue([
        { id: "older", title: "Fix terminal focus", mtime: 100 },
        { id: "newer", title: "Build launch page", mtime: 200 },
    ]);
    mocks.usage.mockResolvedValue({
        provider: "codex",
        plan: "pro",
        windows: [
            { label: "5h", usedPercent: 37, resetsAt: Math.floor(Date.now() / 1000) + 90 * 60, windowMinutes: 300 },
            { label: "7d", usedPercent: 12, resetsAt: Math.floor(Date.now() / 1000) + 4 * 86_400, windowMinutes: 10_080 },
        ],
    });
    invalidate((kind) => kind === "agents.catalog" || kind === "agents.sessions" || kind === "agents.usage");
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("agent rail", () => {
    it("owns recent chats and filters them in place", async () => {
        const user = userEvent.setup();
        render(<AgentRailBody />);

        expect(await screen.findByRole("button", { name: /Fix terminal focus/ })).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Filter recent chats" }));
        await user.type(screen.getByRole("textbox", { name: "Filter recent chats" }), "terminal");

        expect(screen.getByRole("button", { name: /Fix terminal focus/ })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /Build launch page/ })).not.toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: /Fix terminal focus/ }));
        await waitFor(() => expect(getState().agentsBySession["sess-project"]).toHaveLength(1));
        const agent = getState().agents[getState().agentsBySession["sess-project"][0]];
        expect(agent).toMatchObject({ resumeId: "older", title: "Fix terminal focus", cwd: "/code/sikemux" });
    });

    it("shows live plan windows only for detected Codex and Claude providers", async () => {
        const resetBase = Math.floor(Date.now() / 1000);
        mocks.available.mockResolvedValue([
            { type: "codex", label: "Codex", command: "codex", defaultModel: null, defaultEffort: null },
            { type: "claude", label: "Claude", command: "claude", defaultModel: null, defaultEffort: null },
        ]);
        mocks.usage.mockImplementation(async (provider: "codex" | "claude") =>
            provider === "codex"
                ? {
                      provider,
                      plan: "pro",
                      windows: [{ label: "5h", usedPercent: 37, resetsAt: resetBase + 90 * 60, windowMinutes: 300 }],
                  }
                : {
                      provider,
                      plan: "max",
                      windows: [{ label: "7d", usedPercent: 82, resetsAt: "2026-08-20T00:00:00Z", windowMinutes: 10_080 }],
                  },
        );
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");

        const user = userEvent.setup();
        render(<AgentRailBody />);

        expect(await screen.findByRole("region", { name: "Codex plan limits" })).toBeInTheDocument();
        expect(await screen.findByRole("meter", { name: "5h usage" })).toHaveAttribute("aria-valuenow", "37");
        expect(screen.getByText("reset 1h 30m")).toBeInTheDocument();

        await user.click(screen.getByRole("tab", { name: "Claude" }));
        expect(await screen.findByRole("region", { name: "Claude plan limits" })).toBeInTheDocument();
        expect(await screen.findByRole("meter", { name: "7d usage" })).toHaveAttribute("aria-valuenow", "82");
        expect(mocks.usage).toHaveBeenCalledWith("codex", "codex", undefined);
        expect(mocks.usage).toHaveBeenCalledWith("claude", "claude", undefined);
    });

    it("explains unavailable subscription limits without rendering a zero meter", async () => {
        mocks.usage.mockResolvedValue({
            provider: "codex",
            plan: null,
            windows: [],
            unavailableReason: "API-key accounts do not provide plan usage.",
        });
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");

        render(<AgentRailBody />);

        expect(await screen.findByText("API-key accounts do not provide plan usage.")).toBeInTheDocument();
        expect(screen.queryByRole("meter")).not.toBeInTheDocument();
    });

    it("does not request or render plan usage for other detected agents", async () => {
        mocks.available.mockResolvedValue([{ type: "hermes", label: "Hermes", command: "hermes", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");

        render(<AgentRailBody />);

        expect(await screen.findByRole("tab", { name: "Hermes" })).toBeInTheDocument();
        expect(screen.queryByRole("region", { name: /plan limits/i })).not.toBeInTheDocument();
        expect(mocks.usage).not.toHaveBeenCalled();
    });
});
