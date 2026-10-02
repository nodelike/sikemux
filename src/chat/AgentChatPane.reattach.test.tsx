import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AcpAttachment, AcpEvent } from "../api/acp";
import { resetResourcesForTests } from "../state/resources";
import type { Agent } from "../state/types";
import { AgentChatPane } from "./AgentChatPane";
import { chatClaimed } from "./chatClaims";

const mocks = vi.hoisted(() => ({
    listener: null as ((event: AcpEvent) => void) | null,
    attach: vi.fn(),
    start: vi.fn(async () => ({ sessionId: "session-new", capabilities: {}, setup: {} })),
    stop: vi.fn(async () => {}),
    attachAgentSession: vi.fn(),
}));

vi.mock("../api/agents", () => ({ agentApi: { sessionContext: async () => null, available: async () => [] } }));
vi.mock("../api/fs", () => ({ fsapi: { pathKinds: async (paths: string[]) => paths.map(() => null) } }));
vi.mock("../api/git", () => ({ git: { worktrees: async () => [], overview: async () => ({ status: { branch: "main" } }) } }));
vi.mock("../api/acp", () => ({
    acpApi: {
        subscribe: vi.fn(async (listener: (event: AcpEvent) => void) => {
            mocks.listener = listener;
            return () => {};
        }),
        attach: mocks.attach,
        start: mocks.start,
        stop: mocks.stop,
        setPermissionMode: vi.fn(async () => {}),
        setConfig: vi.fn(),
        prompt: vi.fn(async () => {}),
        steer: vi.fn(),
        cancel: vi.fn(async () => {}),
        stopTask: vi.fn(),
        permissionReply: vi.fn(async () => {}),
    },
}));
vi.mock("../state/commands", () => ({
    attachAgentSession: mocks.attachAgentSession,
    setAgentPermissionMode: vi.fn(),
    setAgentModelPreferences: vi.fn(),
    setAgentTitle: vi.fn(),
    titleAgentFromPrompt: vi.fn(),
    noteAcpAgentState: vi.fn(),
    noteAgentBackgroundWork: vi.fn(),
    toggleAgentSkipPermissions: vi.fn(),
    setAgentWorktree: vi.fn(),
    setAgentWorktreeDefault: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

const agent: Agent = {
    id: "agent-kept",
    type: "claude",
    title: "Agent",
    startup: "claude",
    permissionMode: "workspace-write",
    launchState: "live",
    cwd: "/repo",
    resumeId: "session-old",
};

const live: AcpAttachment = {
    status: "live",
    start: { sessionId: "session-kept", capabilities: {}, setup: {} },
    permissionMode: "workspace-write",
    running: true,
    turned: true,
};

function emit(kind: AcpEvent["kind"], payload: Record<string, unknown> = {}) {
    act(() => mocks.listener?.({ agentId: agent.id, kind, payload }));
}

function said(kind: string, text: string) {
    return { sessionId: "session-kept", update: { sessionUpdate: kind, content: { type: "text", text } } };
}

/* What the core replays before it answers: the chat so far, a turn still
   running and the permission request it waits on. */
function replayKeptChat() {
    emit("session_update", { updates: [said("user_message_chunk", "Earlier question"), said("agent_message_chunk", "Earlier answer")] });
    emit("turn_completed", { stopReason: "end_turn" });
    emit("turn_started");
    emit("session_update", { updates: [said("agent_message_chunk", "Still working on it")] });
    emit("permission_request", {
        requestId: "request-1",
        sessionId: "session-kept",
        toolCall: { toolCallId: "call-1", title: "Touch a file" },
        options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
    });
}

/* jsdom measures everything as nothing, and the transcript keeps its rows
   out of the DOM until its scroller has a size and says so. */
const resizeWatchers = new Map<Element, Set<ResizeObserverCallback>>();

class TestResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(target: Element) {
        resizeWatchers.set(target, (resizeWatchers.get(target) ?? new Set()).add(this.callback));
    }
    unobserve(target: Element) {
        resizeWatchers.get(target)?.delete(this.callback);
    }
    disconnect() {
        resizeWatchers.forEach((watchers) => watchers.delete(this.callback));
    }
}

/* A live row shows its text a few words at a time, in pieces. */
const transcript = () => document.querySelector(".chat-scroll")?.textContent ?? "";
const showsText = (text: string) => waitFor(() => expect(transcript()).toContain(text));

function renderPane() {
    const view = render(<AgentChatPane agent={agent} cwd="/repo" active visible onBusyChange={() => {}} />);
    const scroller = document.querySelector(".chat-scroll") as HTMLElement;
    for (const [name, size] of [
        ["offsetWidth", 600],
        ["offsetHeight", 400],
        ["clientHeight", 400],
    ] as const)
        Object.defineProperty(scroller, name, { configurable: true, get: () => size });
    const target: Element = scroller;
    const entries = [{ target } as ResizeObserverEntry];
    act(() => resizeWatchers.get(scroller)?.forEach((callback) => callback(entries, {} as ResizeObserver)));
    return view;
}

beforeEach(() => {
    vi.clearAllMocks();
    resetResourcesForTests();
    mocks.listener = null;
    resizeWatchers.clear();
    globalThis.ResizeObserver = TestResizeObserver as unknown as typeof ResizeObserver;
});

afterEach(cleanup);

describe("AgentChatPane with a chat the core kept", () => {
    it("takes the running chat up where it is instead of starting another", async () => {
        mocks.attach.mockImplementation(async () => {
            replayKeptChat();
            return live;
        });
        renderPane();
        await showsText("Still working on it");
        expect(transcript()).toContain("Earlier question");
        expect(transcript()).toContain("Earlier answer");
        expect(screen.getByRole("region", { name: "Permission required for Touch a file" })).toBeInTheDocument();
        expect(mocks.attach).toHaveBeenCalledWith({ agentId: agent.id, provider: "claude", cwd: "/repo", configPath: undefined });
        expect(mocks.start).not.toHaveBeenCalled();
        expect(mocks.stop).not.toHaveBeenCalled();
        expect(mocks.attachAgentSession).toHaveBeenCalledWith(agent.id, "session-kept");
        expect(chatClaimed(agent.id)).toBe(true);

        emit("session_update", { updates: [said("agent_message_chunk", " and done")] });
        await showsText("Still working on it and done");
    });

    it("starts the chat on its saved session when the core has none", async () => {
        mocks.attach.mockResolvedValue({ status: "missing" });
        renderPane();
        await waitFor(() => expect(mocks.start).toHaveBeenCalledWith(expect.objectContaining({ agentId: agent.id, resumeId: "session-old" })));
        expect(mocks.stop).not.toHaveBeenCalled();
    });

    it("starts again from the provider's history when the core kept too much to replay", async () => {
        mocks.attach.mockResolvedValue({ status: "restart" });
        renderPane();
        await waitFor(() => expect(mocks.start).toHaveBeenCalledWith(expect.objectContaining({ resumeId: "session-old" })));
        expect(mocks.stop).toHaveBeenCalledWith(agent.id);
        expect(mocks.stop.mock.invocationCallOrder[0]).toBeLessThan(mocks.start.mock.invocationCallOrder[0]);
    });

    it("takes the chat up again when the core comes back with it, without stopping it", async () => {
        mocks.attach.mockImplementation(async () => {
            replayKeptChat();
            return live;
        });
        renderPane();
        await showsText("Still working on it");

        emit("reattach");
        await waitFor(() => expect(mocks.attach).toHaveBeenCalledTimes(2));
        await showsText("Still working on it");
        expect(transcript().split("Earlier answer")).toHaveLength(2);
        expect(mocks.stop).not.toHaveBeenCalled();
        expect(mocks.start).not.toHaveBeenCalled();
    });

    it("stops the chat when the pane goes away", async () => {
        mocks.attach.mockResolvedValue(live);
        const view = renderPane();
        await waitFor(() => expect(mocks.attach).toHaveBeenCalled());
        view.unmount();
        await waitFor(() => expect(mocks.stop).toHaveBeenCalledWith(agent.id));
    });
});
