import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
    available: vi.fn(),
    saved: vi.fn(),
    recent: vi.fn(),
    usage: vi.fn(),
    renameSession: vi.fn(() => Promise.resolve()),
}));

vi.mock("../api/agents", () => ({
    agentApi: { available: mocks.available, recent: mocks.recent, usage: mocks.usage, renameSession: mocks.renameSession },
}));

interface Saved {
    id: string;
    title: string;
    mtime: number;
    project?: string;
    agent?: string;
}

/* Pages the saved chats the way the backend does: every requested provider
   and project, newest first, a cursor carrying on from the last row. */
function fakeRecent(request: RecentChatsRequest): Promise<RecentChatsPage> {
    const rows = request.providers
        .flatMap((provider) =>
            (mocks.saved() as Saved[])
                .filter((row) => !row.agent || row.agent === provider.agent)
                .map((row) => ({ agent: provider.agent, id: row.id, title: row.title, mtime: row.mtime, project: row.project ?? "/code/sikemux" })),
        )
        .filter((row) => request.projects.includes(row.project))
        .filter((row) => !request.exclude.some((open) => open.agent === row.agent && open.id === row.id))
        .filter((row) => !request.query || row.title.toLowerCase().includes(request.query))
        .sort((a, b) => b.mtime - a.mtime);
    const start = request.cursor ? Number(request.cursor.key) : 0;
    const page = rows.slice(start, start + request.limit);
    const end = start + page.length;
    return Promise.resolve({
        sessions: page,
        next: page.length === request.limit && end < rows.length ? { atMs: 0, agent: "", key: String(end) } : null,
    });
}

// jsdom has no ResizeObserver; the rail uses one to keep filling its list.
vi.stubGlobal(
    "ResizeObserver",
    class {
        observe() {}
        disconnect() {}
    },
);

import type { RecentChatsPage, RecentChatsRequest } from "../api/agents";
import { invalidate } from "../state/resources";
import { getState, setState } from "../state/store";
import { AgentRailBody } from "./AgentRail";
import { agentIdsOf } from "../state/selectors";
import { withAgents } from "../test/agents";

function openAgent(title: string) {
    setState((state) =>
        withAgents(state, "sess-project", [{ id: "agent-open", type: "codex", title, startup: "codex", cwd: "/code/sikemux", launchState: "live" }]),
    );
}

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
                pinned: false,
                activeWindowId: "win-project",
            },
        },
        sessionOrder: ["sess-project"],
        activeSessionId: "sess-project",
        agents: {},
    });
    mocks.available.mockResolvedValue([{ type: "codex", label: "Codex", command: "codex", defaultModel: "gpt-5.6-sol", defaultEffort: "high" }]);
    mocks.saved.mockReturnValue([
        { id: "older", title: "Fix terminal focus", mtime: 100 },
        { id: "newer", title: "Build launch page", mtime: 200 },
    ]);
    mocks.recent.mockImplementation(fakeRecent);
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
        await waitFor(() => expect(agentIdsOf(getState(), "sess-project")).toHaveLength(1));
        const agent = getState().agents[agentIdsOf(getState(), "sess-project")[0]];
        expect(agent).toMatchObject({ resumeId: "older", title: "Fix terminal focus", cwd: "/code/sikemux" });
    });

    it("starts a fresh chat for the selected provider from the new chat row", async () => {
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.click(await screen.findByRole("button", { name: "New chat" }));
        await waitFor(() => expect(agentIdsOf(getState(), "sess-project")).toHaveLength(1));
        const agent = getState().agents[agentIdsOf(getState(), "sess-project")[0]];
        expect(agent).toMatchObject({ type: "codex", cwd: "/code/sikemux" });
        expect(agent.resumeId).toBeUndefined();
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

    it("renames an open chat in place on double-click", async () => {
        openAgent("Fix terminal focus");
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.dblClick(await screen.findByRole("button", { name: "Fix terminal focus" }));
        const field = screen.getByRole("textbox", { name: "Chat name" });
        await user.clear(field);
        await user.type(field, "Terminal focus bug{Enter}");

        expect(getState().agents["agent-open"]).toMatchObject({ title: "Terminal focus bug", renamed: true });
        expect(screen.getByRole("button", { name: "Terminal focus bug" })).toBeInTheDocument();
    });

    it("keeps the old name when a rename is cancelled with Escape", async () => {
        openAgent("Fix terminal focus");
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.dblClick(await screen.findByRole("button", { name: "Fix terminal focus" }));
        await user.type(screen.getByRole("textbox", { name: "Chat name" }), " draft{Escape}");

        expect(getState().agents["agent-open"].title).toBe("Fix terminal focus");
        expect(screen.queryByRole("textbox", { name: "Chat name" })).not.toBeInTheDocument();
    });

    it("opens the agent menu on right-click and renames from it", async () => {
        openAgent("Fix terminal focus");
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: "Fix terminal focus" }) });
        expect(screen.getByRole("menuitem", { name: /Close Others/ })).toBeInTheDocument();
        expect(screen.getByRole("menuitem", { name: /Copy Link/ })).toBeInTheDocument();

        await user.click(screen.getByRole("menuitem", { name: "Rename…" }));
        expect(screen.getByRole("textbox", { name: "Chat name" })).toHaveValue("Fix terminal focus");
    });

    it("renames a recent chat in the provider's own session from the row's menu", async () => {
        mocks.available.mockResolvedValue([{ type: "claude", label: "Claude", command: "claude", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog");
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: /Fix terminal focus/ }) });
        await user.click(screen.getByRole("menuitem", { name: "Rename…" }));
        const field = screen.getByRole("textbox", { name: "Chat name" });
        await user.clear(field);
        await user.type(field, "Terminal focus bug{Enter}");

        expect(mocks.renameSession).toHaveBeenCalledWith("claude", "/code/sikemux", "older", "Terminal focus bug", "claude", undefined);
        expect(agentIdsOf(getState(), "sess-project")).toHaveLength(0);
    });

    it("opens a recent chat from the row's menu", async () => {
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: /Fix terminal focus/ }) });
        expect(screen.getByRole("menuitem", { name: "Rename…" })).toBeInTheDocument();

        await user.click(screen.getByRole("menuitem", { name: "Open" }));
        await waitFor(() => expect(agentIdsOf(getState(), "sess-project")).toHaveLength(1));
    });

    it("lists every provider's chats from the all agents tab and starts a chat through the picker", async () => {
        mocks.available.mockResolvedValue([
            { type: "codex", label: "Codex", command: "codex", defaultModel: null, defaultEffort: null },
            { type: "claude", label: "Claude", command: "claude", defaultModel: null, defaultEffort: null },
        ]);
        mocks.saved.mockReturnValue([
            { id: "c1", title: "Codex chat", mtime: 100, agent: "codex" },
            { id: "k1", title: "Claude chat", mtime: 200, agent: "claude" },
        ]);
        invalidate((kind) => kind === "agents.catalog");
        const user = userEvent.setup();
        render(<AgentRailBody />);

        expect(await screen.findByRole("button", { name: /Codex chat/ })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /Claude chat/ })).not.toBeInTheDocument();

        await user.click(screen.getByRole("tab", { name: "All agents" }));
        expect(await screen.findByRole("button", { name: /Claude chat/ })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /Codex chat/ })).toBeInTheDocument();
        expect(getState().agentRailAllAgents).toBe(true);
        expect(screen.queryByRole("region", { name: /plan limits/i })).not.toBeInTheDocument();

        await user.click(screen.getByRole("tab", { name: "Claude" }));
        expect(getState().agentRailAllAgents).toBe(false);
        await waitFor(() => expect(screen.queryByRole("button", { name: /Codex chat/ })).not.toBeInTheDocument());
    });

    it("shows every project's open agents by what they need, and their chats, under all projects", async () => {
        setState((state) => ({
            sessions: {
                ...state.sessions,
                "sess-other": {
                    id: "sess-other",
                    name: "website",
                    kind: "project" as const,
                    cwd: "/code/website",
                    pinned: false,
                    activeWindowId: "win-other",
                },
            },
            sessionOrder: ["sess-project", "sess-other"],
        }));
        setState((state) =>
            withAgents(state, "sess-other", [
                { id: "agent-away", type: "codex", title: "Pricing copy", startup: "codex", cwd: "/code/website", launchState: "live" },
            ]),
        );
        setState((state) => ({
            agentActivity: {
                ...state.agentActivity,
                "agent-away": {
                    state: "blocked",
                    backendState: "blocked",
                    unread: false,
                    updatedAt: 0,
                    sequence: 1,
                    source: "acp",
                    confidence: "high",
                    reason: "",
                },
            },
        }));
        mocks.saved.mockReturnValue([
            { id: "here", title: "Fix terminal focus", mtime: 100 },
            { id: "there", title: "Hero image", mtime: 300, project: "/code/website" },
        ]);
        const user = userEvent.setup();
        render(<AgentRailBody />);

        expect(await screen.findByRole("button", { name: /Fix terminal focus/ })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /Hero image/ })).not.toBeInTheDocument();

        await user.click(screen.getByRole("tab", { name: "All projects, 1 waiting" }));
        expect(await screen.findByRole("button", { name: /Hero image/ })).toBeInTheDocument();
        expect(screen.getByText("Needs you 1")).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "New chat" })).not.toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: /Hero image/ }));
        await waitFor(() => expect(getState().activeSessionId).toBe("sess-other"));
        expect(agentIdsOf(getState(), "sess-other").map((id) => getState().agents[id].resumeId)).toContain("there");
    });

    it("closes an open agent from the all projects list", async () => {
        openAgent("Ship the fix");
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.click(await screen.findByRole("tab", { name: /All projects/ }));
        await user.click(screen.getByRole("button", { name: "Close Ship the fix" }));
        await waitFor(() => expect(agentIdsOf(getState(), "sess-project")).not.toContain("agent-open"));
    });

    it("shows no account picker when the provider has one account", async () => {
        render(<AgentRailBody />);

        expect(await screen.findByRole("region", { name: "Codex plan limits" })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "Codex account" })).not.toBeInTheDocument();
    });

    it("switches the account from the limits footer and reads the new account's limits and chats", async () => {
        mocks.available.mockImplementation(async (profiles: { type: string; configPath?: string }[]) => [
            {
                type: "codex",
                label: "Codex",
                command: "codex",
                configPath: profiles.find((profile) => profile.type === "codex")?.configPath ?? null,
                defaultModel: null,
                defaultEffort: null,
            },
        ]);
        setState((state) => ({
            providerProfiles: [
                ...state.providerProfiles,
                { id: "codex-work", name: "Work", provider: "codex", accent: "#7a9dff", configPath: "~/.codex-work" },
            ],
        }));
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");
        const user = userEvent.setup();
        render(<AgentRailBody />);

        expect(await screen.findByRole("button", { name: "Codex account" })).toHaveTextContent("Codex");
        await waitFor(() => expect(mocks.usage).toHaveBeenCalledWith("codex", "codex", undefined));

        await user.click(screen.getByRole("button", { name: "Codex account" }));
        await user.click(screen.getByRole("option", { name: /Work/ }));

        expect(getState().selectedProviderProfileIds.codex).toBe("codex-work");
        await waitFor(() => expect(mocks.usage).toHaveBeenCalledWith("codex", "codex", "~/.codex-work"));
        await waitFor(() =>
            expect(mocks.recent).toHaveBeenCalledWith(expect.objectContaining({ providers: [{ agent: "codex", configPath: "~/.codex-work" }] })),
        );
        expect(screen.getByRole("button", { name: "Codex account" })).toHaveTextContent("Work");
        expect(screen.getByRole("tab", { name: "Codex" })).toHaveAttribute("aria-selected", "true");
    });

    it("opens the agents settings page from the account picker", async () => {
        setState((state) => ({
            providerProfiles: [
                ...state.providerProfiles,
                { id: "codex-work", name: "Work", provider: "codex", accent: "#7a9dff", configPath: "~/.codex-work" },
            ],
        }));
        const user = userEvent.setup();
        render(<AgentRailBody />);

        await user.click(await screen.findByRole("button", { name: "Codex account" }));
        await user.click(screen.getByRole("option", { name: "Manage accounts…" }));

        expect(getState()).toMatchObject({ settingsOpen: true, settingsPage: "agents" });
        expect(getState().selectedProviderProfileIds.codex).toBe("builtin-codex");
    });

    it("asks for the next page as the list scrolls to its end", async () => {
        mocks.saved.mockReturnValue(Array.from({ length: 30 }, (_, index) => ({ id: `s${index}`, title: `Chat ${index}`, mtime: 1000 - index })));
        render(<AgentRailBody />);

        expect(await screen.findByText("Chat 0")).toBeInTheDocument();
        await waitFor(() => expect(mocks.recent.mock.calls.length).toBeGreaterThan(1));
        const cursors = mocks.recent.mock.calls.map((call) => (call[0] as RecentChatsRequest).cursor?.key ?? null);
        expect(cursors.slice(0, 3)).toEqual([null, "12", "24"]);
        expect(await screen.findByText("Chat 29")).toBeInTheDocument();
    });
});
