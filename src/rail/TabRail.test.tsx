import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
    available: vi.fn(),
    saved: vi.fn(),
    recent: vi.fn(),
    usage: vi.fn(),
    account: vi.fn(),
    addAccount: vi.fn(),
    signIn: vi.fn(),
    renameSession: vi.fn(() => Promise.resolve()),
    deleteSession: vi.fn(() => Promise.resolve()),
    inbox: vi.fn(),
}));

vi.mock("./useInbox", async (original) => ({ ...(await original<typeof import("./useInbox")>()), useInbox: () => mocks.inbox() }));

vi.mock("../api/agents", () => ({
    agentApi: {
        available: mocks.available,
        recent: mocks.recent,
        usage: mocks.usage,
        account: mocks.account,
        addAccount: mocks.addAccount,
        signIn: mocks.signIn,
        forgetAccount: () => Promise.resolve(),
        onSignInPage: () => Promise.resolve(() => {}),
        renameSession: mocks.renameSession,
        deleteSession: mocks.deleteSession,
        selectAccount: async (provider: string, id: string) => {
            const { getState } = await import("../state/store");
            const state = getState();
            return {
                profiles: state.providerProfiles,
                selected: { ...state.selectedProviderProfileIds, [provider]: id },
                autoSwitch: state.accountAutoSwitch,
            };
        },
    },
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
import { acceptDialog, dismissDialog, useDialogs } from "../state/dialog";
import { invalidate } from "../state/resources";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { AccountDialogHost } from "../agents/AccountDialog";
import { TabRailBody } from "./TabRail";
import { agentIdsOf } from "../state/selectors";
import { withAgents } from "../test/agents";

function openAgent(title: string) {
    setState((state) => withAgents(state, "sess-project", [{ id: "agent-open", type: "codex", title, cwd: "/code/sikemux", launchState: "live" }]));
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
    mocks.account.mockImplementation(async (_agent: string, configPath?: string) => ({
        signedIn: true,
        name: null,
        email: configPath ? "work@example.com" : "me@example.com",
        plan: null,
        organization: null,
        method: "subscription",
        sessions: "/home/me/.codex/sessions",
    }));
    mocks.signIn.mockResolvedValue(undefined);
    mocks.inbox.mockReturnValue({ inbox: { items: [], working: 0, away: false, remoteAgents: [], agents: [] }, digest: null });
    invalidate((kind) => kind === "agents.catalog" || kind === "agents.sessions" || kind === "agents.usage" || kind === "agents.account");
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("tab rail", () => {
    it("owns recent chats and filters them in place", async () => {
        const user = userEvent.setup();
        render(<TabRailBody />);

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

    it("offers to set up an agent when it finds none", async () => {
        mocks.available.mockResolvedValue([]);
        mocks.saved.mockReturnValue([]);
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.click(await screen.findByRole("button", { name: "Set one up" }));
        expect(getState()).toMatchObject({ settingsOpen: true, settingsPage: "agents", settingsTarget: "Agents on this machine" });
    });

    it("starts a fresh chat for the selected provider from the new chat row", async () => {
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.click(await screen.findByRole("button", { name: /^New chat/ }));
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
        render(<TabRailBody />);

        expect(await screen.findByRole("region", { name: "Codex plan limits" })).toBeInTheDocument();
        expect(await screen.findByRole("meter", { name: "5h usage" })).toHaveAttribute("aria-valuenow", "37");
        expect(screen.getByText("1h 30m")).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Agent provider" }));
        await user.click(screen.getByRole("option", { name: /^Claude/ }));
        expect(await screen.findByRole("region", { name: "Claude plan limits" })).toBeInTheDocument();
        expect(await screen.findByRole("meter", { name: "7d usage" })).toHaveAttribute("aria-valuenow", "82");
        expect(mocks.usage).toHaveBeenCalledWith("codex", undefined);
        expect(mocks.usage).toHaveBeenCalledWith("claude", undefined);
    });

    it("explains unavailable subscription limits without rendering a zero meter", async () => {
        mocks.usage.mockResolvedValue({
            provider: "codex",
            plan: null,
            windows: [],
            unavailableReason: "API-key accounts do not provide plan usage.",
        });
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");

        render(<TabRailBody />);

        expect(await screen.findByText("API-key accounts do not provide plan usage.")).toBeInTheDocument();
        expect(screen.queryByRole("meter")).not.toBeInTheDocument();
    });

    it("does not request or render plan usage for other detected agents", async () => {
        mocks.available.mockResolvedValue([{ type: "hermes", label: "Hermes", command: "hermes", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");

        render(<TabRailBody />);

        expect(await screen.findByText("Hermes")).toBeInTheDocument();
        expect(screen.queryByRole("region", { name: /plan limits/i })).not.toBeInTheDocument();
        expect(mocks.usage).not.toHaveBeenCalled();
    });

    it("renames an open chat in place on double-click", async () => {
        openAgent("Fix terminal focus");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.dblClick(await screen.findByRole("button", { name: "Fix terminal focus" }));
        const field = screen.getByRole("textbox", { name: "Title" });
        await user.clear(field);
        await user.type(field, "Terminal focus bug{Enter}");

        expect(getState().agents["agent-open"]).toMatchObject({ title: "Terminal focus bug", renamed: true });
        expect(screen.getByRole("button", { name: "Terminal focus bug" })).toBeInTheDocument();
    });

    it("keeps the old name when a rename is cancelled with Escape", async () => {
        openAgent("Fix terminal focus");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.dblClick(await screen.findByRole("button", { name: "Fix terminal focus" }));
        await user.type(screen.getByRole("textbox", { name: "Title" }), " draft{Escape}");

        expect(getState().agents["agent-open"].title).toBe("Fix terminal focus");
        expect(screen.queryByRole("textbox", { name: "Title" })).not.toBeInTheDocument();
    });

    it("opens the agent menu on right-click and renames from it", async () => {
        openAgent("Fix terminal focus");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: "Fix terminal focus" }) });
        expect(screen.getByRole("menuitem", { name: /Close Others/ })).toBeInTheDocument();
        expect(screen.getByRole("menuitem", { name: /Copy Link/ })).toBeInTheDocument();

        await user.click(screen.getByRole("menuitem", { name: "Edit Title…" }));
        expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("Fix terminal focus");
    });

    it("renames a recent chat in the provider's own session from the row's menu", async () => {
        mocks.available.mockResolvedValue([{ type: "claude", label: "Claude", command: "claude", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: /Fix terminal focus/ }) });
        await user.click(screen.getByRole("menuitem", { name: "Rename…" }));
        const field = screen.getByRole("textbox", { name: "Title" });
        await user.clear(field);
        await user.type(field, "Terminal focus bug{Enter}");

        expect(mocks.renameSession).toHaveBeenCalledWith("claude", "/code/sikemux", "older", "Terminal focus bug", "claude", undefined);
        expect(agentIdsOf(getState(), "sess-project")).toHaveLength(0);
    });

    it("deletes a recent chat for good once the person confirms", async () => {
        mocks.available.mockResolvedValue([{ type: "claude", label: "Claude", command: "claude", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: /Fix terminal focus/ }) });
        await user.click(screen.getByRole("menuitem", { name: "Delete…" }));
        expect(useDialogs.getState().dialog).toMatchObject({ kind: "confirm", title: "Delete this chat?", destructive: true });
        expect(mocks.deleteSession).not.toHaveBeenCalled();

        act(() => acceptDialog(useDialogs.getState().dialog!.id));

        await waitFor(() => expect(screen.queryByRole("button", { name: /Fix terminal focus/ })).not.toBeInTheDocument());
        expect(mocks.deleteSession).toHaveBeenCalledWith("claude", "/code/sikemux", "older", "claude", undefined);
        expect(screen.getByRole("button", { name: /Build launch page/ })).toBeInTheDocument();
    });

    it("keeps a recent chat when the delete is cancelled", async () => {
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: /Fix terminal focus/ }) });
        await user.click(screen.getByRole("menuitem", { name: "Delete…" }));
        act(() => dismissDialog(useDialogs.getState().dialog!.id));

        await waitFor(() => expect(useDialogs.getState().dialog).toBeNull());
        expect(mocks.deleteSession).not.toHaveBeenCalled();
        expect(screen.getByRole("button", { name: /Fix terminal focus/ })).toBeInTheDocument();
    });

    it("offers no delete for chats of agents other than Claude and Codex", async () => {
        mocks.available.mockResolvedValue([{ type: "opencode", label: "OpenCode", command: "opencode", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: /Fix terminal focus/ }) });
        expect(screen.getByRole("menuitem", { name: "Rename…" })).toBeInTheDocument();
        expect(screen.queryByRole("menuitem", { name: "Delete…" })).not.toBeInTheDocument();
    });

    it("keeps a pinned chat at the top, open or closed, until it is unpinned", async () => {
        const user = userEvent.setup();
        render(<TabRailBody />);
        const pinned = () => within(screen.getByText("Pinned").closest<HTMLElement>(".agent-group")!);

        await user.pointer({ keys: "[MouseRight]", target: await screen.findByRole("button", { name: /Fix terminal focus/ }) });
        await user.click(screen.getByRole("menuitem", { name: "Pin" }));
        await waitFor(() => expect(screen.getAllByRole("button", { name: /Fix terminal focus/ })).toHaveLength(1));
        expect(pinned().getByRole("button", { name: /Fix terminal focus/ })).toBeInTheDocument();

        await user.click(pinned().getByRole("button", { name: /Fix terminal focus/ }));
        await waitFor(() => expect(agentIdsOf(getState(), "sess-project")).toHaveLength(1));
        const [id] = agentIdsOf(getState(), "sess-project");
        expect(pinned().getByRole("button", { name: "Fix terminal focus" })).toBeInTheDocument();

        act(() => cmd.renameAgent(id, "Terminal focus bug"));
        act(() => cmd.closeAgent(id));
        await waitFor(() => expect(agentIdsOf(getState(), "sess-project")).toHaveLength(0));
        expect(pinned().getByRole("button", { name: /Terminal focus bug/ })).toBeInTheDocument();

        await user.pointer({ keys: "[MouseRight]", target: pinned().getByRole("button", { name: /Terminal focus bug/ }) });
        await user.click(screen.getByRole("menuitem", { name: "Unpin" }));
        expect(screen.queryByText("Pinned")).not.toBeInTheDocument();
        expect(await screen.findByRole("button", { name: /Fix terminal focus/ })).toBeInTheDocument();
    });

    it("opens a recent chat from the row's menu", async () => {
        const user = userEvent.setup();
        render(<TabRailBody />);

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
        render(<TabRailBody />);

        expect(await screen.findByRole("button", { name: /Codex chat/ })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /Claude chat/ })).not.toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Agent provider" }));
        await user.click(screen.getByRole("option", { name: "All harnesses" }));
        expect(await screen.findByRole("button", { name: /Claude chat/ })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /Codex chat/ })).toBeInTheDocument();
        expect(getState().tabRailAllAgents).toBe(true);
        expect(screen.queryByRole("region", { name: /plan limits/i })).not.toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Agent provider" }));
        await user.click(screen.getByRole("option", { name: /^Claude/ }));
        expect(getState().tabRailAllAgents).toBe(false);
        await waitFor(() => expect(screen.queryByRole("button", { name: /Codex chat/ })).not.toBeInTheDocument());
    });

    it("shows every project's open agents and their chats, and what waits elsewhere, under all projects", async () => {
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
            withAgents(state, "sess-other", [{ id: "agent-away", type: "codex", title: "Pricing copy", cwd: "/code/website", launchState: "live" }]),
        );
        mocks.inbox.mockReturnValue({
            inbox: {
                items: [
                    {
                        id: "question:q1",
                        tier: "needs",
                        kind: "question",
                        agentId: "agent-away",
                        name: null,
                        cli: "codex",
                        project: "/code/website",
                        title: "Pricing copy",
                        what: "Asks you",
                        detail: "Which plan goes first?",
                        at: 0,
                        card: null,
                        requestId: null,
                        choices: null,
                        questionId: "q1",
                        url: null,
                        handoff: null,
                        recordings: [],
                    },
                ],
                working: 0,
                away: false,
                remoteAgents: [],
                agents: [],
            },
            digest: null,
        });
        mocks.saved.mockReturnValue([
            { id: "here", title: "Fix terminal focus", mtime: 100 },
            { id: "there", title: "Hero image", mtime: 300, project: "/code/website" },
        ]);
        const user = userEvent.setup();
        render(<TabRailBody />);

        expect(await screen.findByRole("button", { name: /Fix terminal focus/ })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /Hero image/ })).not.toBeInTheDocument();

        await user.click(screen.getByRole("tab", { name: "All projects, 1 waiting" }));
        expect(await screen.findByRole("button", { name: /Hero image/ })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /^New chat/ })).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: /Hero image/ }));
        await waitFor(() => expect(getState().activeSessionId).toBe("sess-other"));
        expect(agentIdsOf(getState(), "sess-other").map((id) => getState().agents[id].resumeId)).toContain("there");
    });

    it("closes an open agent from the all projects list", async () => {
        openAgent("Ship the fix");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.click(await screen.findByRole("tab", { name: /All projects/ }));
        await user.click(screen.getByRole("button", { name: "Close Ship the fix" }));
        await waitFor(() => expect(agentIdsOf(getState(), "sess-project")).not.toContain("agent-open"));
    });

    it("goes by the name on the account until the person names it", async () => {
        mocks.account.mockImplementation(async (_agent: string, configPath?: string) => ({
            signedIn: true,
            name: configPath ? "Someone" : "Kishore",
            email: configPath ? "work@example.com" : "me@example.com",
            plan: null,
            organization: null,
            method: "subscription",
            sessions: "/home/me/.codex/sessions",
        }));
        setState((state) => ({
            providerProfiles: [
                ...state.providerProfiles,
                { id: "codex-work", name: "Work", provider: "codex", accent: "#7a9dff", configPath: "~/.codex-work" },
            ],
        }));
        const user = userEvent.setup();
        render(<TabRailBody />);

        await user.click(await screen.findByRole("button", { name: "Codex account: Kishore" }));
        expect(await screen.findByRole("menuitem", { name: /^Work/ })).toBeInTheDocument();
    });

    it("starts new chats on another account from the account menu and reads its limits and chats", async () => {
        mocks.available.mockImplementation(async () => {
            const { getState } = await import("../state/store");
            const state = getState();
            const chosen = state.providerProfiles.find((profile) => profile.id === state.selectedProviderProfileIds.codex);
            return [
                { type: "codex", label: "Codex", command: "codex", configPath: chosen?.configPath ?? null, defaultModel: null, defaultEffort: null },
            ];
        });
        setState((state) => ({
            providerProfiles: [
                ...state.providerProfiles,
                { id: "codex-work", name: "Work", provider: "codex", accent: "#7a9dff", configPath: "~/.codex-work" },
            ],
        }));
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");
        const user = userEvent.setup();
        render(<TabRailBody />);

        await waitFor(() => expect(mocks.usage).toHaveBeenCalledWith("codex", undefined));
        await user.click(await screen.findByRole("button", { name: "Codex account: Codex" }));
        await user.click(screen.getByRole("menuitem", { name: /^Work/ }));

        await waitFor(() => expect(getState().selectedProviderProfileIds.codex).toBe("codex-work"));
        await waitFor(() => expect(mocks.usage).toHaveBeenCalledWith("codex", "~/.codex-work"));
        await waitFor(() =>
            expect(mocks.recent).toHaveBeenCalledWith(expect.objectContaining({ providers: [{ agent: "codex", configPath: "~/.codex-work" }] })),
        );
        expect(await screen.findByRole("button", { name: "Codex account: Work" })).toBeInTheDocument();
        expect(await screen.findByTitle(/^work@example.com/)).toBeInTheDocument();
    });

    it("signs a signed-out account in from the rail", async () => {
        mocks.account.mockResolvedValue({
            signedIn: false,
            name: null,
            email: null,
            plan: null,
            organization: null,
            method: null,
            sessions: "/home/me/.codex/sessions",
        });
        const user = userEvent.setup();
        render(
            <>
                <TabRailBody />
                <AccountDialogHost />
            </>,
        );

        await user.click(await screen.findByRole("button", { name: "Sign in" }));
        expect(screen.getByRole("dialog", { name: "Sign in to Codex" })).toBeInTheDocument();
        await waitFor(() => expect(mocks.signIn).toHaveBeenCalled());
    });

    it("adds an account from the rail, and never offers to remove the built-in one", async () => {
        const user = userEvent.setup();
        render(
            <>
                <TabRailBody />
                <AccountDialogHost />
            </>,
        );

        await user.click(await screen.findByRole("button", { name: "Codex account: Codex" }));
        expect(screen.queryByRole("menuitem", { name: /^Remove/ })).not.toBeInTheDocument();
        await user.click(screen.getByRole("menuitem", { name: "Add account…" }));

        expect(screen.getByRole("dialog", { name: "Add a Codex account" })).toBeInTheDocument();
    });

    it("asks for the next page as the list scrolls to its end", async () => {
        mocks.saved.mockReturnValue(Array.from({ length: 30 }, (_, index) => ({ id: `s${index}`, title: `Chat ${index}`, mtime: 1000 - index })));
        render(<TabRailBody />);

        expect(await screen.findByText("Chat 0")).toBeInTheDocument();
        await waitFor(() => expect(mocks.recent.mock.calls.length).toBeGreaterThan(1));
        const cursors = mocks.recent.mock.calls.map((call) => (call[0] as RecentChatsRequest).cursor?.key ?? null);
        expect(cursors.slice(0, 3)).toEqual([null, "12", "24"]);
        expect(await screen.findByText("Chat 29")).toBeInTheDocument();
    });
});
