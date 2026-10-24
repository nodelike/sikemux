import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AcpAttachment, AcpEvent } from "../api/acp";
import type { ChatChange, ChatMessage, ChatPart, ChatView } from "./types";
import { moveChat } from "../agents/accounts";
import { resetResourcesForTests } from "../state/resources";
import type { Agent, ProviderProfile } from "../state/types";
import { typeDraft } from "../test/draft";
import { AgentChatPane } from "./AgentChatPane";

const mocks = vi.hoisted(() => ({
    listener: null as ((event: AcpEvent) => void) | null,
    attach: vi.fn(),
    startChat: vi.fn(async () => {}),
    prompt: vi.fn(async () => {}),
    stop: vi.fn(async () => {}),
    switchAccount: vi.fn(async () => {}),
}));

vi.mock("../api/agents", () => ({
    agentApi: { available: async () => [], account: async () => ({ signedIn: true, sessions: null }) },
}));
vi.mock("../api/fs", () => {
    const fsapi = { pathKinds: async (paths: string[]) => paths.map(() => null) };
    return { fsapi, filesOn: () => fsapi };
});
vi.mock("../api/git", () => ({
    git: {
        worktrees: async () => [],
        overview: async () => ({ status: { branch: "main" } }),
        projectRepo: async (project: string) => ({ repo: project, repos: null }),
    },
}));
vi.mock("../api/acp", () => ({
    acpApi: {
        composing: vi.fn(async () => {}),
        subscribe: vi.fn(async (listener: (event: AcpEvent) => void) => {
            mocks.listener = listener;
            return () => {};
        }),
        attach: mocks.attach,
        startChat: mocks.startChat,
        stop: mocks.stop,
        switchAccount: mocks.switchAccount,
        setPermissionMode: vi.fn(async () => {}),
        setConfig: vi.fn(),
        prompt: mocks.prompt,
        steer: vi.fn(),
        cancel: vi.fn(async () => {}),
        stopTask: vi.fn(),
        permissionReply: vi.fn(async () => {}),
    },
}));
vi.mock("../state/workspaceSent", () => ({ workspaceSent: async () => {} }));
vi.mock("../state/commands", () => ({
    setAgentPermissionMode: vi.fn(),
    setAgentTitle: vi.fn(),
    titleAgentFromPrompt: vi.fn(),
    toggleAgentSkipPermissions: vi.fn(),
    setAgentWorktree: vi.fn(),
    setAgentWorktreeDefault: vi.fn(),
}));

const agent: Agent = {
    id: "agent-kept",
    type: "claude",
    title: "Agent",
    permissionMode: "workspace-write",
    launchState: "live",
    cwd: "/repo",
    resumeId: "session-old",
};

const text = (id: string, said: string): ChatPart => ({ kind: "text", id, text: said });
const row = (id: string, role: ChatMessage["role"], said: string): ChatMessage => ({ id, role, parts: [text(`${id}.0`, said)] });

/* The chat as the core kept it: a finished turn, a turn still running and the permission request it waits on. */
const keptChat: ChatView = {
    head: {
        sessionId: "session-kept",
        connection: "ready",
        running: true,
        turned: true,
        permissionMode: "workspace-write",
        commands: [],
        capabilities: {},
        setup: {},
        permissions: [
            {
                requestId: "request-1",
                sessionId: "session-kept",
                toolCall: { toolCallId: "call-1", title: "Touch a file", row: { action: "other", target: "Touch a file" } },
                options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
            },
        ],
        tasks: [],
        runningSubagents: [],
        queued: [],
        earlierLost: false,
    },
    rows: [row("m1", "user", "Earlier question"), row("m2", "assistant", "Earlier answer"), row("m3", "assistant", "Still working on it")],
};

/* The view is the chat as of change 2; changes numbered past it follow. */
const live: AcpAttachment = { status: "live", view: keptChat, feed: "feed-1", seq: 2 };

function emit(event: { kind: "reattach" } | { kind: "changes"; seq: number; payload: ChatChange[] }) {
    act(() => mocks.listener?.({ agentId: agent.id, payload: null, ...event } as AcpEvent));
}

const appended = (seq: number, said: string) => emit({ kind: "changes", seq, payload: [{ op: "text", row: "m3", part: "m3.0", text: said }] });

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
    measure();
    return view;
}

function measure() {
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
            appended(2, " already held");
            appended(3, " and");
            return live;
        });
        renderPane();
        await showsText("Still working on it and");
        expect(transcript()).not.toContain("already held");
        expect(transcript()).toContain("Earlier question");
        expect(transcript()).toContain("Earlier answer");
        expect(screen.getByRole("region", { name: "Permission required for Touch a file" })).toBeInTheDocument();
        expect(mocks.attach).toHaveBeenCalledWith(agent.id);
        expect(mocks.startChat).not.toHaveBeenCalled();
        expect(mocks.stop).not.toHaveBeenCalled();

        appended(4, " done");
        await showsText("Still working on it and done");
    });

    it("leaves a chat whose agent is not running as it is, and takes it up once a message woke it", async () => {
        mocks.attach.mockResolvedValueOnce({ status: "missing" }).mockResolvedValue(live);
        renderPane();
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor.getAttribute("aria-placeholder")).toContain("Ask about this project"));
        expect(mocks.startChat).not.toHaveBeenCalled();
        typeDraft(editor, "Carry on");
        fireEvent.keyDown(editor, { key: "Enter" });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "Carry on", [], [], expect.any(String)));
        await waitFor(() => expect(mocks.attach).toHaveBeenCalledTimes(2));
        expect(mocks.startChat).not.toHaveBeenCalled();
        expect(mocks.stop).not.toHaveBeenCalled();
    });

    it("takes the chat up again when the core comes back with it, without stopping it", async () => {
        mocks.attach.mockResolvedValue(live);
        renderPane();
        await showsText("Still working on it");

        emit({ kind: "reattach" });
        await waitFor(() => expect(mocks.attach).toHaveBeenCalledTimes(2));
        await showsText("Still working on it");
        expect(transcript().split("Earlier answer")).toHaveLength(2);
        expect(mocks.stop).not.toHaveBeenCalled();
        expect(mocks.startChat).not.toHaveBeenCalled();
    });

    it("leaves the chat running when the pane goes away", async () => {
        mocks.attach.mockResolvedValue(live);
        const view = renderPane();
        await waitFor(() => expect(mocks.attach).toHaveBeenCalled());
        view.unmount();
        await act(async () => {});
        expect(mocks.stop).not.toHaveBeenCalled();
    });

    it("takes a chat moved to another account up where it is, mid-turn, without stopping it", async () => {
        mocks.attach.mockResolvedValue(live);
        const view = renderPane();
        await showsText("Still working on it");

        const work: ProviderProfile = { id: "work", name: "Work", provider: "claude", accent: "#000", configPath: "~/.claude-work" };
        moveChat(agent, work);
        expect(mocks.switchAccount).toHaveBeenCalledWith(agent.id, "claude", { id: "work", label: "Work", configPath: "~/.claude-work" });
        view.rerender(<AgentChatPane agent={{ ...agent, profileId: "work" }} profile={work} cwd="/repo" active visible onBusyChange={() => {}} />);
        measure();
        await act(async () => {});

        expect(mocks.attach).toHaveBeenCalledTimes(1);
        await showsText("Still working on it");
        expect(mocks.stop).not.toHaveBeenCalled();
        expect(mocks.startChat).not.toHaveBeenCalled();
    });
});
