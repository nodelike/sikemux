import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { acpApi, type AcpEvent } from "../api/acp";
import { IS_MACOS } from "../lib/platform";
import { dispatchPathDrop } from "../state/dropRegistry";
import type { Agent } from "../state/types";
import { AgentChatPane } from "./AgentChatPane";
import { shownImage } from "../state/imageViewer";
import { forgetPathState } from "./pathExistence";

const mocks = vi.hoisted(() => ({
    eventListener: null as ((event: AcpEvent) => void) | null,
    prompt: vi.fn(async () => {}),
    steer: vi.fn(async () => "injected"),
    setPermissionMode: vi.fn(async () => {}),
    stopTask: vi.fn(async () => {}),
    setConfig: vi.fn(),
    setAgentModelPreferences: vi.fn(),
    attachAgentSession: vi.fn(),
    setAgentPermissionMode: vi.fn(),
    noteAcpAgentState: vi.fn(),
    noteAgentBackgroundWork: vi.fn(),
    start: vi.fn(async () => ({ sessionId: "session-1", capabilities: {}, setup: {} })),
    pathKinds: vi.fn(async (paths: string[]): Promise<(string | null)[]> => paths.map(() => null)),
    revealInFinder: vi.fn(async () => {}),
    requestOpenFile: vi.fn(),
    sessionContext: vi.fn(async (): Promise<{ used: number; size: number | null } | null> => null),
}));

vi.mock("../api/agents", () => ({ agentApi: { sessionContext: mocks.sessionContext } }));

vi.mock("../api/fs", () => ({
    fsapi: {
        pathKinds: mocks.pathKinds,
        revealInFinder: mocks.revealInFinder,
        readFileBase64: vi.fn(async () => {
            throw new Error("no file");
        }),
    },
}));

vi.mock("../api/acp", () => ({
    acpApi: {
        subscribe: vi.fn(async (listener: (event: AcpEvent) => void) => {
            mocks.eventListener = listener;
            return () => {
                mocks.eventListener = null;
            };
        }),
        start: mocks.start,
        setPermissionMode: mocks.setPermissionMode,
        setConfig: mocks.setConfig,
        stop: vi.fn(async () => {}),
        prompt: mocks.prompt,
        steer: mocks.steer,
        cancel: vi.fn(async () => {}),
        stopTask: mocks.stopTask,
        permissionReply: vi.fn(async () => {}),
    },
}));

vi.mock("../state/commands", () => ({
    requestOpenFile: mocks.requestOpenFile,
    attachAgentSession: mocks.attachAgentSession,
    setAgentPermissionMode: mocks.setAgentPermissionMode,
    setAgentModelPreferences: mocks.setAgentModelPreferences,
    setAgentTitle: vi.fn(),
    noteAcpAgentState: mocks.noteAcpAgentState,
    noteAgentBackgroundWork: mocks.noteAgentBackgroundWork,
    toggleAgentSkipPermissions: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

const agent: Agent = {
    id: "agent-1",
    type: "codex",
    title: "Agent session",
    startup: "codex",
    permissionMode: "workspace-write",
    launchState: "live",
};

const shortcutKey = { key: "Enter", modifier: IS_MACOS ? { metaKey: true } : { ctrlKey: true } };

function emit(kind: AcpEvent["kind"], payload: Record<string, unknown>): void {
    // Streamed updates arrive a frame's worth at a time.
    act(() => mocks.eventListener?.({ agentId: agent.id, kind, payload: kind === "session_update" ? { updates: [payload] } : payload }));
}

function emitBatch(updates: Record<string, unknown>[]): void {
    act(() => mocks.eventListener?.({ agentId: agent.id, kind: "session_update", payload: { updates } }));
}

// jsdom reports no sizes and never fires a resize, so a scroller and the
// observer watching it both have to be played by hand. Callbacks are kept per
// target: the transcript virtualizer watches its own rows and must not be
// handed a resize meant for the scroll content.
const resizeCallbacks = new Map<Element, Set<ResizeObserverCallback>>();

class TestResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(target: Element) {
        const watchers = resizeCallbacks.get(target) ?? new Set<ResizeObserverCallback>();
        watchers.add(this.callback);
        resizeCallbacks.set(target, watchers);
    }
    unobserve(target: Element) {
        resizeCallbacks.get(target)?.delete(this.callback);
    }
    disconnect() {
        resizeCallbacks.forEach((watchers) => watchers.delete(this.callback));
    }
}

const nextFrame = () => act(async () => new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve())));

function reportResize(target: Element) {
    const entries = [{ target } as ResizeObserverEntry];
    const observer = {} as ResizeObserver;
    act(() => resizeCallbacks.get(target)?.forEach((callback) => callback(entries, observer)));
}

function fakeScroller(element: HTMLElement, clientHeight: number) {
    let scrollTop = 0;
    let scrollHeight = clientHeight;
    Object.defineProperty(element, "clientHeight", { configurable: true, get: () => clientHeight });
    Object.defineProperty(element, "scrollHeight", { configurable: true, get: () => scrollHeight });
    Object.defineProperty(element, "scrollTop", {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => {
            scrollTop = value;
        },
    });
    return {
        scrollTo(top: number) {
            fireEvent.wheel(element);
            scrollTop = top;
            fireEvent.scroll(element);
        },
        // The transcript moving itself, with no reader behind it.
        driftTo(top: number) {
            scrollTop = top;
            fireEvent.scroll(element);
        },
        grow(height: number) {
            scrollHeight = height;
            const content = element.querySelector(".chat-scroll-content");
            if (content) reportResize(content);
            fireEvent.scroll(element);
        },
    };
}

beforeEach(() => {
    vi.clearAllMocks();
    // Nothing a transcript names is a real file unless a test says it is.
    forgetPathState();
    mocks.pathKinds.mockImplementation(async (paths: string[]) => paths.map(() => null));
    mocks.eventListener = null;
    resizeCallbacks.clear();
    globalThis.ResizeObserver = TestResizeObserver as unknown as typeof ResizeObserver;
});

afterEach(cleanup);

/* The virtualizer keeps a row out of the DOM until the scroller has a size,
   and jsdom measures everything as nothing. */
async function openTranscript(prompt = "Look at the styles"): Promise<void> {
    render(<AgentChatPane agent={{ ...agent, model: "gpt-6-astra" }} cwd="/repo" active visible onBusyChange={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message agent" }) as HTMLTextAreaElement;
    await waitFor(() => expect(editor.placeholder).toContain("Ask about this project"));
    fireEvent.change(editor, { target: { value: prompt } });
    fireEvent.keyDown(editor, { key: "Enter" });
    const scroller = document.querySelector(".chat-scroll") as HTMLElement;
    fakeScroller(scroller, 400);
    Object.defineProperty(scroller, "offsetWidth", { configurable: true, get: () => 600 });
    Object.defineProperty(scroller, "offsetHeight", { configurable: true, get: () => 400 });
    reportResize(scroller);
    await screen.findByRole("status");
}

describe("AgentChatPane", () => {
    it("keeps receiving hidden session updates while freezing transcript rendering", async () => {
        const props = { agent, cwd: "/repo", active: true, visible: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        rerender(<AgentChatPane {...props} visible={false} />);
        emit("permission_request", {
            requestId: "hidden-request",
            sessionId: "session-1",
            toolCall: { toolCallId: "tool-1" },
            options: [{ optionId: "allow", name: "Allow hidden tool", kind: "allow_once" }],
        });
        expect(screen.queryByRole("button", { name: "Allow hidden tool" })).not.toBeInTheDocument();
        expect(acpApi.stop).not.toHaveBeenCalled();

        rerender(<AgentChatPane {...props} />);
        expect(await screen.findByRole("button", { name: "Allow hidden tool" })).toBeInTheDocument();
    });

    it("shows a resumed session's saved context until the agent reports its own", async () => {
        mocks.sessionContext.mockResolvedValueOnce({ used: 84_000, size: null });
        mocks.start.mockResolvedValueOnce({
            sessionId: "session-1",
            capabilities: {},
            setup: { configOptions: [{ id: "model", type: "select", currentValue: "opus[1m]", options: [] }] },
        });
        const claude: Agent = { ...agent, type: "claude", startup: "claude", resumeId: "saved-1" };
        render(<AgentChatPane agent={claude} cwd="/repo" active onBusyChange={() => {}} />);

        expect(await screen.findByRole("img", { name: "Context window 8% used" })).toBeInTheDocument();
        expect(mocks.sessionContext).toHaveBeenCalledWith("claude", "/repo", "saved-1", undefined);

        emit("session_update", { sessionId: "session-1", update: { sessionUpdate: "usage_update", used: 150_000, size: 200_000 } });
        expect(await screen.findByRole("img", { name: "Context window 75% used" })).toBeInTheDocument();
    });

    it("keeps the harness editable for a loaded session without messages", async () => {
        render(<AgentChatPane agent={{ ...agent, resumeId: "empty-session" }} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(screen.getByRole("button", { name: "Model" })).toBeEnabled());
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        expect(screen.getByRole("group", { name: "Agent" })).toBeInTheDocument();
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Existing message" } },
        });
        emit("ready", { capabilities: {}, setup: {} });
        await waitFor(() => expect(screen.queryByRole("group", { name: "Agent" })).not.toBeInTheDocument());
    });

    it("leaves focus in an open model menu when the session state changes", async () => {
        const props = { agent, cwd: "/repo", active: true, visible: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} />);
        await waitFor(() => expect(screen.getByRole("button", { name: "Model" })).toBeEnabled());
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const search = screen.getByRole("combobox", { name: "Search model" });
        await waitFor(() => expect(search).toHaveFocus());
        rerender(<AgentChatPane {...props} visible={false} />);
        rerender(<AgentChatPane {...props} />);
        await new Promise((resolve) => window.requestAnimationFrame(resolve));
        expect(search).toHaveFocus();
        expect(screen.getByRole("group", { name: "Agent" })).toBeInTheDocument();
    });

    it("changes the model live and persists only the confirmed configuration", async () => {
        const configs = (model: string) => [
            {
                id: "model",
                name: "Model",
                type: "select",
                currentValue: model,
                options: [
                    { value: "astra", name: "GPT-6 Astra" },
                    { value: "sol", name: "GPT-5.6 Sol" },
                ],
            },
            { id: "reasoning_effort", name: "Effort", type: "select", currentValue: "high", options: [{ value: "high", name: "High" }] },
        ];
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: {}, setup: { configOptions: configs("astra") } });
        mocks.setConfig.mockResolvedValueOnce({ configOptions: configs("sol") });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(screen.getByRole("button", { name: "Model" })).toBeEnabled());
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.change(screen.getByRole("combobox", { name: "Search model" }), { target: { value: "Sol" } });
        fireEvent.keyDown(screen.getByRole("combobox", { name: "Search model" }), { key: "Enter" });
        await waitFor(() => expect(mocks.setConfig).toHaveBeenCalledWith(agent.id, "model", "sol"));
        await waitFor(() => expect(mocks.setAgentModelPreferences).toHaveBeenCalledWith(agent.id, "sol", "high"));
        expect(screen.getByRole("button", { name: "Model" })).toHaveTextContent("GPT-5.6 Sol");
        expect(mocks.start).toHaveBeenCalledTimes(1);
        expect(acpApi.stop).not.toHaveBeenCalled();
    });

    it("attaches a new session when its first turn starts so metadata can update while it runs", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(screen.getByRole("textbox", { name: "Message agent" })).toBeEnabled());
        expect(mocks.attachAgentSession).not.toHaveBeenCalled();

        emit("turn_started", {});

        expect(mocks.attachAgentSession).toHaveBeenCalledWith(agent.id, "session-1");
    });

    it("changes YOLO on the live session without restarting", async () => {
        const props = { agent, cwd: "/repo", active: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} />);
        await waitFor(() => expect(screen.getByRole("textbox", { name: "Message agent" })).toBeEnabled());

        rerender(<AgentChatPane {...props} agent={{ ...agent, permissionMode: "bypass" }} />);
        await waitFor(() => expect(mocks.setPermissionMode).toHaveBeenCalledWith(agent.id, "bypass"));
        expect(mocks.start).toHaveBeenCalledTimes(1);
        expect(acpApi.stop).not.toHaveBeenCalled();
    });

    it("leaves YOLO to the TUI once the chat session has stopped", async () => {
        const props = { agent, cwd: "/repo", active: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} />);
        await waitFor(() => expect(screen.getByRole("textbox", { name: "Message agent" })).toBeEnabled());
        rerender(<AgentChatPane {...props} active={false} />);

        rerender(<AgentChatPane {...props} active={false} agent={{ ...agent, permissionMode: "bypass" }} />);
        await act(async () => {});

        expect(mocks.setPermissionMode).not.toHaveBeenCalled();
        expect(mocks.setAgentPermissionMode).not.toHaveBeenCalled();
    });

    it("serializes rapid permission changes and applies the latest choice", async () => {
        let complete!: () => void;
        mocks.setPermissionMode.mockImplementationOnce(
            () =>
                new Promise<void>((resolve) => {
                    complete = resolve;
                }),
        );
        const props = { agent, cwd: "/repo", active: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} />);
        await waitFor(() => expect(screen.getByRole("textbox", { name: "Message agent" })).toBeEnabled());
        rerender(<AgentChatPane {...props} agent={{ ...agent, permissionMode: "bypass" }} />);
        await waitFor(() => expect(complete).toBeDefined());
        rerender(<AgentChatPane {...props} />);
        await act(async () => complete());
        await waitFor(() => expect(mocks.setPermissionMode).toHaveBeenLastCalledWith(agent.id, "workspace-write"));
        expect(mocks.setPermissionMode).toHaveBeenCalledTimes(2);
        expect(mocks.start).toHaveBeenCalledTimes(1);
    });

    it("passes the configured executable, model and effort to ACP without restarting for equivalent profile arrays", async () => {
        const profile = {
            id: "custom",
            provider: "codex" as const,
            name: "Custom",
            accent: "#888888",
            executablePath: "/custom/codex",
            environmentKeys: ["CUSTOM_KEY"],
        };
        const props = { agent: { ...agent, model: "custom-model", effort: "high" as const }, cwd: "/repo", active: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} profile={profile} />);
        await waitFor(() =>
            expect(mocks.start).toHaveBeenCalledWith(
                expect.objectContaining({ executablePath: "/custom/codex", model: "custom-model", effort: "high" }),
            ),
        );
        rerender(<AgentChatPane {...props} profile={{ ...profile, environmentKeys: ["CUSTOM_KEY"] }} />);
        await act(async () => {});
        expect(mocks.start).toHaveBeenCalledTimes(1);
    });

    it("does not launch after a delayed subscription resolves on an unmounted pane", async () => {
        let subscribed!: () => void;
        vi.mocked(acpApi.subscribe).mockImplementationOnce(
            () =>
                new Promise((resolve) => {
                    subscribed = () => resolve(() => {});
                }),
        );
        const { unmount } = render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(subscribed).toBeDefined());
        unmount();
        await act(async () => subscribed());
        expect(mocks.start).not.toHaveBeenCalled();
    });

    it("rolls back a rejected permission update and keeps the conversation connected", async () => {
        const props = { agent, cwd: "/repo", active: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} />);
        await waitFor(() => expect(screen.getByRole("textbox", { name: "Message agent" })).toBeEnabled());
        mocks.setPermissionMode.mockRejectedValueOnce(new Error("Mode unavailable"));
        rerender(<AgentChatPane {...props} agent={{ ...agent, permissionMode: "bypass" }} />);
        expect(await screen.findByText("Mode unavailable")).toBeInTheDocument();
        expect(mocks.setAgentPermissionMode).toHaveBeenCalledWith(agent.id, "workspace-write");
        expect(mocks.start).toHaveBeenCalledTimes(1);
    });

    it("sends one prompt at a time while the first is waiting for turn_started", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Second" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(mocks.prompt).toHaveBeenCalledTimes(1);
        expect(screen.getByRole("button", { name: "Stop agent" })).toBeInTheDocument();
    });

    it("holds a message written mid-turn until the running turn ends", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Then look at the tests" } });
        fireEvent.keyDown(editor, { key: "Enter" });

        expect(await screen.findByLabelText("1 queued")).toHaveTextContent("Then look at the tests");
        expect(mocks.steer).not.toHaveBeenCalled();
        expect(mocks.prompt).toHaveBeenCalledTimes(1);

        emit("turn_completed", {});

        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "Then look at the tests", []));
        expect(screen.queryByLabelText("1 queued")).not.toBeInTheDocument();
    });

    it("offers no steering for an agent that cannot take a message mid-turn", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Second" } });
        fireEvent.keyDown(editor, { key: shortcutKey.key, ...shortcutKey.modifier });

        expect(await screen.findByLabelText("1 queued")).toHaveTextContent("Second");
        expect(mocks.steer).not.toHaveBeenCalled();
    });

    it("puts a queued message into the running turn when its steer is asked for", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Actually, check the other file" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(mocks.steer).not.toHaveBeenCalled();

        fireEvent.click(await screen.findByRole("button", { name: "Steer the running turn with Actually, check the other file" }));

        await waitFor(() => expect(mocks.steer).toHaveBeenCalledWith(agent.id, "Actually, check the other file", []));
        expect(mocks.prompt).toHaveBeenCalledTimes(1);
        expect(screen.queryByLabelText("1 queued")).not.toBeInTheDocument();
    });

    it("steers straight from the composer on the shortcut", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Stop, wrong file" } });
        fireEvent.keyDown(editor, { key: shortcutKey.key, ...shortcutKey.modifier });

        await waitFor(() => expect(mocks.steer).toHaveBeenCalledWith(agent.id, "Stop, wrong file", []));
        expect(screen.queryByLabelText("1 queued")).not.toBeInTheDocument();
    });

    it("steers the one queued message on the shortcut when nothing is drafted", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Actually, check the other file" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        await screen.findByLabelText("1 queued");
        expect(mocks.steer).not.toHaveBeenCalled();

        fireEvent.keyDown(editor, { key: shortcutKey.key, ...shortcutKey.modifier });

        await waitFor(() => expect(mocks.steer).toHaveBeenCalledWith(agent.id, "Actually, check the other file", []));
        expect(screen.queryByLabelText("1 queued")).not.toBeInTheDocument();
    });

    /* Steering aborts the turn, so the shortcut must not guess which message
       it takes when there is more than one to choose from. */
    it("leaves a queue of two alone on the shortcut", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Then the tests" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "And the rail" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        await screen.findByLabelText("2 queued");

        fireEvent.keyDown(editor, { key: shortcutKey.key, ...shortcutKey.modifier });

        expect(mocks.steer).not.toHaveBeenCalled();
        expect(await screen.findByLabelText("2 queued")).toBeInTheDocument();
    });

    it("sends as its own prompt when the turn ended before the steer arrived", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        mocks.steer.mockResolvedValueOnce("promptRequired");
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "First" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.change(editor, { target: { value: "Carry on" } });
        fireEvent.keyDown(editor, { key: shortcutKey.key, ...shortcutKey.modifier });

        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "Carry on", []));
    });

    it("keeps saying the turn is alive, and names the tool it is running", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "Check the tests" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(await screen.findByRole("status")).toHaveTextContent("Thinking…");

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "mcp__github__list_issues", status: "in_progress" },
        });
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("list_issues"));

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "completed" },
        });
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Thinking…"));

        emit("turn_completed", { stopReason: "end_turn" });
        await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    });

    it("reads every update in one streamed frame", async () => {
        await openTranscript();
        emitBatch([
            { sessionId: "session-1", update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "One " } } },
            { sessionId: "session-1", update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "two " } } },
            { sessionId: "session-1", update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "three" } } },
        ]);
        expect(await screen.findByText("One two three")).toBeInTheDocument();
    });

    it("offers a copy on the prompt and on the answer it drew", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "agent_message_chunk", messageId: "m1", content: { type: "text", text: "The pane paints no background" } },
        });

        expect(await screen.findByRole("button", { name: "Copy message" })).toBeInTheDocument();
        expect(await screen.findByRole("button", { name: "Copy reply" })).toBeInTheDocument();
    });

    it("says what a running tool is doing instead of quoting the command it was given", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "Check the styles" } });
        fireEvent.keyDown(editor, { key: "Enter" });

        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-1",
                kind: "execute",
                title: 'grep -n "is-transparent" -A12 src/styles/base.css | head -40',
                status: "in_progress",
            },
        });
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Running a command…"));
        expect(screen.getByRole("status")).not.toHaveTextContent("is-transparent");
    });

    it("holds a run of tool calls open through the gaps between them, and folds it once the turn ends", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "Weighing the two options" } },
        });
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call", toolCallId: "tool-1", kind: "read", title: "src/styles/chat.css", status: "in_progress" },
        });

        expect(await screen.findByText("Weighing the two options")).toBeVisible();
        const run = await screen.findByRole("button", { name: /1 tool call/ });
        expect(run).toHaveAttribute("aria-expanded", "true");

        // The turn is still going, so the wait for the next call is not a fold.
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "completed" },
        });
        expect(run).toHaveAttribute("aria-expanded", "true");

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call", toolCallId: "tool-2", kind: "read", title: "src/styles/base.css", status: "in_progress" },
        });
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call_update", toolCallId: "tool-2", status: "completed" },
        });
        const finished = await screen.findByRole("button", { name: /2 tool calls/ });
        expect(finished).toHaveAttribute("aria-expanded", "true");
        expect(document.querySelectorAll(".chat-tool")).toHaveLength(2);

        emit("turn_completed", { stopReason: "end_turn" });
        await waitFor(() => expect(finished).toHaveAttribute("aria-expanded", "false"));
        expect(document.querySelectorAll(".chat-tool")).toHaveLength(0);

        fireEvent.click(finished);
        expect(finished).toHaveAttribute("aria-expanded", "true");
        expect(document.querySelectorAll(".chat-tool")).toHaveLength(2);
    });

    it("builds a subagent's transcript only once it is opened", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "subagent_spawned", subagentSessionId: "subagent-1", name: "Explorer", task: "Read the styles" },
        });
        emit("session_update", {
            sessionId: "subagent-1",
            update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Found the pane" } },
        });

        await waitFor(() => expect(document.querySelector("details.chat-subagent")).not.toBeNull());
        const folded = document.querySelector("details.chat-subagent") as HTMLDetailsElement;
        expect(folded.querySelector(".chat-subagent-body")).toBeNull();
        expect(folded.textContent).toContain("Explorer");

        act(() => {
            folded.open = true;
            fireEvent(folded, new Event("toggle"));
        });
        expect(await screen.findByText("Found the pane")).toBeInTheDocument();
    });

    it("folds a finished run away when the agent moves on to something else in the same turn", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call", toolCallId: "tool-1", kind: "read", title: "src/styles/chat.css", status: "completed" },
        });
        const run = await screen.findByRole("button", { name: /1 tool call/ });
        expect(run).toHaveAttribute("aria-expanded", "true");

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "The pane paints no background" } },
        });
        await waitFor(() => expect(run).toHaveAttribute("aria-expanded", "false"));
    });

    it("hangs each call off the run as a kind, a target and how long it took", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-1",
                kind: "execute",
                title: "pnpm vitest run src/lib/shaderField.test.ts",
                status: "in_progress",
            },
        });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-2",
                kind: "read",
                title: "src/components/browser/BrowserPane.tsx",
                status: "completed",
            },
        });

        const rows = await screen.findAllByTitle(/shaderField|BrowserPane/);
        expect(rows[0]).toHaveTextContent("run");
        expect(rows[0]).toHaveTextContent("pnpm vitest run src/lib/shaderField.test.ts");
        // A path shows the name it ends in; the whole path stays in the tooltip.
        expect(rows[1]).toHaveTextContent("BrowserPane.tsx");
        expect(rows[1]).not.toHaveTextContent("src/components");
        expect(rows[1]).toHaveAttribute("title", "src/components/browser/BrowserPane.tsx");
    });

    it("opens the file a call touched, and still opens what the call did", async () => {
        mocks.pathKinds.mockImplementation(async (paths: string[]) => paths.map(() => "file"));
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-1",
                kind: "edit",
                title: "src/styles/stage.css",
                status: "completed",
                locations: [{ path: "/repo/src/styles/stage.css", line: 2 }],
                content: [
                    {
                        type: "diff",
                        path: "src/styles/stage.css",
                        oldText: ".stage {\n    background: var(--pane);\n}\n",
                        newText: ".stage {\n    background: transparent;\n}\n",
                    },
                ],
            },
        });

        const file = await waitFor(() => {
            const chip = document.querySelector(".chat-file-ref");
            expect(chip).not.toBeNull();
            return chip as HTMLElement;
        });
        expect(file).toHaveTextContent("stage.css");
        fireEvent.click(file);
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/repo/src/styles/stage.css", 1, undefined);

        fireEvent.click(screen.getByRole("button", { name: /Show what the call did/ }));
        await waitFor(() => expect(document.querySelector(".chat-diff-line.add")).not.toBeNull());
    });

    it("opens an edit onto the hunk it wrote, and a failure onto why", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-1",
                kind: "edit",
                title: "src/styles/stage.css",
                status: "completed",
                content: [
                    {
                        type: "diff",
                        path: "src/styles/stage.css",
                        oldText: ".stage {\n    background: var(--pane);\n}\n",
                        newText: ".stage {\n    background: transparent;\n}\n",
                    },
                ],
            },
        });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-2",
                kind: "execute",
                title: "pnpm vitest run",
                status: "failed",
                rawOutput: { output: "1 failed · expected rgba(26,22,36,.72)" },
            },
        });

        const edit = await screen.findByTitle("src/styles/stage.css");
        expect(edit).toHaveTextContent("+1");
        expect(edit).toHaveTextContent("−1");
        expect(screen.queryByText(/background: transparent;/)).not.toBeInTheDocument();

        fireEvent.click(edit);
        const added = await waitFor(() => document.querySelector(".chat-diff-line.add") as HTMLElement);
        expect(added).toHaveTextContent("background: transparent;");
        // Only the part that changed is marked, not the whole line.
        expect(added.querySelector("mark")).toHaveTextContent("transparent");
        expect(document.querySelector(".chat-diff-line.del")).toHaveTextContent("background: var(--pane);");

        fireEvent.click(screen.getByTitle("pnpm vitest run"));
        expect(await screen.findByText(/1 failed/)).toBeInTheDocument();
    });

    it("opens a picture an agent sent, with a name to save it under", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "image", mimeType: "image/png", data: "SEVMTE8=" },
            },
        });

        const picture = await screen.findByRole("img", { name: "attachment.png" });
        expect(picture).toHaveAttribute("src", "data:image/png;base64,SEVMTE8=");

        fireEvent.click(picture);
        expect(shownImage()).toEqual({ src: "data:image/png;base64,SEVMTE8=", name: "attachment.png", path: undefined });
    });

    it("names an mcp call for the server it went to, and gives the column room for it", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "mcp__atlassian__addCommentToJiraIssue", status: "in_progress" },
        });

        const row = await screen.findByTitle("mcp__atlassian__addCommentToJiraIssue");
        expect(row).toHaveTextContent("atlassian");
        expect(row).toHaveTextContent("addCommentToJiraIssue");
        expect(row).toHaveAttribute("data-kind", "mcp");
        expect((document.querySelector(".chat-tools-body") as HTMLElement).style.getPropertyValue("--chat-kind")).toBe("9ch");
    });

    it("rules a table the agent wrote, and lets a wide one scroll on its own", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "| approach | cpu |\n| --- | --- |\n| ack every frame | 90.6% |\n" },
            },
        });

        const cell = await screen.findByText("ack every frame");
        expect(cell.tagName).toBe("TD");
        expect(screen.getByText("approach").tagName).toBe("TH");
        expect(cell.closest(".chat-table")).not.toBeNull();
    });

    it("shows markup the person pasted, and still drops markup the agent wrote", async () => {
        await openTranscript('Use this: “<svg viewBox="0 0 16 16"><path d="M0 0h16"/></svg>”\n\n<div>own line</div>');
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Done <b>quietly</b>" } },
        });

        const typed = await screen.findByText(/Use this:/);
        expect(typed.textContent).toBe('Use this: “<svg viewBox="0 0 16 16"><path d="M0 0h16"/></svg>”');
        expect(screen.getByText("<div>own line</div>")).toBeInTheDocument();
        expect(document.querySelector(".chat-message.user .chat-markdown svg")).toBeNull();
        expect(await screen.findByText(/Done/)).toHaveTextContent(/^Done quietly$/);
    });

    it("leaves out the header band when the table has no column labels", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "|  |  |\n| --- | --- |\n| Image write path | live row |\n" },
            },
        });

        const cell = await screen.findByText("Image write path");
        const table = cell.closest("table") as HTMLTableElement;
        expect(table.querySelector("thead")).toBeNull();
        expect(table.querySelectorAll("tr")).toHaveLength(1);
    });

    it("colours a patch the agent wrote in a fence, and leaves ordinary output alone", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "agent_message_chunk",
                content: {
                    type: "text",
                    text: "```\n .stage {\n-    background: var(--pane);\n+    background: transparent;\n }\n```\n\n```\n900x600 ok\n1024x600 FLOOD\n```\n",
                },
            },
        });

        await waitFor(() => expect(document.querySelector(".chat-code-diff")).not.toBeNull());
        expect(document.querySelector(".chat-code-diff .chat-diff-line.del")).toHaveTextContent("background: var(--pane);");
        expect(document.querySelector(".chat-code-diff .chat-diff-line.add mark")).toHaveTextContent("transparent");
        // The block of measurements beside it is not a patch and keeps its own shape.
        expect(document.querySelectorAll(".chat-code-diff")).toHaveLength(1);
    });

    it("watches a working subagent over the composer and settles its card when the turn ends", async () => {
        await openTranscript();
        emit("turn_started", {});
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "subagent_spawned",
                subagentSessionId: "subagent-1",
                name: "Explore",
                task: "You are implementing performance fixes\nin the sikemux desktop app repo",
            },
        });
        emit("session_update", {
            sessionId: "subagent-1",
            update: { sessionUpdate: "tool_call", toolCallId: "tool-1", kind: "search", title: "usePty", status: "in_progress" },
        });

        const strip = await screen.findByLabelText("1 subagent");
        expect(strip).toHaveTextContent("Explore");
        expect(strip).toHaveTextContent("search usePty");

        const card = document.querySelector(".chat-subagent") as HTMLElement;
        expect(card.querySelector('[role="img"][aria-label="working"]')).not.toBeNull();
        expect(card).toHaveTextContent("1 call");
        // The task is a whole prompt, so the row shows its first line only.
        expect(card).toHaveTextContent("You are implementing performance fixes");
        expect(card).not.toHaveTextContent("in the sikemux desktop app repo");

        emit("turn_completed", { stopReason: "cancelled" });

        await waitFor(() => expect(screen.queryByLabelText("1 subagent")).not.toBeInTheDocument());
        expect(document.querySelector('.chat-subagent [role="img"][aria-label="stopped"]')).not.toBeNull();
        expect(document.querySelector(".chat-tool-spinner")).toBeNull();
    });

    it("shows adapter progress and starts a failed adapter again on its own", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());

        emit("status", { state: "installing" });
        expect(screen.getAllByText("Installing structured-session adapter…")[0]).toBeInTheDocument();
        emit("error", { message: "adapter failed" });
        const callsBeforeRetry = mocks.start.mock.calls.length;
        expect(screen.getAllByText("Reconnecting…")[0]).toBeInTheDocument();

        await waitFor(() => expect(mocks.start.mock.calls.length).toBeGreaterThan(callsBeforeRetry), { timeout: 3_000 });
    });

    it("offers reconnecting a dropped transcript in the transcript column", async () => {
        await openTranscript();
        emit("status", { state: "stopped" });

        const notice = await waitFor(() => {
            const element = document.querySelector(".chat-reconnect");
            expect(element).not.toBeNull();
            return element as HTMLElement;
        });
        expect(notice).toHaveTextContent("Reconnecting…");
        // Every control in the transcript belongs to a styled row, never loose in the scroller.
        expect(document.querySelector(".chat-scroll-content > button")).toBeNull();
    });

    it("sends a message written while the session is down once it is back", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active visible profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("status", { state: "stopped" });

        const editor = screen.getByRole("textbox", { name: "Message agent" });
        fireEvent.change(editor, { target: { value: "carry on" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(mocks.prompt).not.toHaveBeenCalled();

        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "carry on", []), { timeout: 3_000 });
    });

    it("shows permission requests even when the adapter omits the optional tool title", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("permission_request", {
            requestId: "request-1",
            sessionId: "session-1",
            toolCall: { toolCallId: "tool-1" },
            options: [{ optionId: "allow", name: "Allow once", kind: "allow_once" }],
        });
        fireEvent.click(await screen.findByRole("button", { name: "Allow once" }));
        await waitFor(() => expect(acpApi.permissionReply).toHaveBeenCalledWith(agent.id, "request-1", "allow"));
    });

    it("lists a background task until it ends, and stops it on request", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "async_task_spawned",
                asyncTaskId: "task-1",
                name: "pnpm test",
                taskType: "shell",
                description: "Run the suite",
                canStop: true,
            },
        });
        expect(await screen.findByRole("button", { name: "Stop pnpm test" })).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Stop pnpm test" }));
        await waitFor(() => expect(mocks.stopTask).toHaveBeenCalledWith("agent-1", "task-1"));

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "async_task_state_update", asyncTaskId: "task-1", state: "stopped" },
        });
        await waitFor(() => expect(screen.queryByText("pnpm test")).not.toBeInTheDocument());
    });

    it("says the agent is still in use while a background task outlives the turn", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "async_task_spawned",
                asyncTaskId: "task-1",
                name: "push gate",
                taskType: "shell",
                description: "Push 20 commits through pre-push gates",
            },
        });
        await waitFor(() => expect(mocks.noteAgentBackgroundWork).toHaveBeenCalledWith("agent-1", 1, 0));

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "async_task_state_update", asyncTaskId: "task-1", state: "completed" },
        });
        await waitFor(() => expect(mocks.noteAgentBackgroundWork).toHaveBeenLastCalledWith("agent-1", 0, 0));
    });

    it("says a background task's name once when its description repeats it", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "async_task_spawned",
                asyncTaskId: "task-1",
                name: "push gate failures",
                description: "push gate failures",
                taskType: "shell",
            },
        });

        const chip = await screen.findByLabelText("1 shell");
        expect(chip).toHaveTextContent("push gate failures");
        expect(chip.querySelector(".chat-task-detail")).toHaveTextContent("shell");
    });

    it("shows ACP slash commands and inserts the selected command", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "available_commands_update",
                availableCommands: [{ name: "compact", description: "Compact context", input: { hint: "focus" } }],
            },
        });
        await act(async () => window.requestAnimationFrame(() => {}));

        const editor = screen.getByRole("textbox", { name: "Message agent" });
        fireEvent.change(editor, { target: { value: "/" } });
        expect(await screen.findByRole("option", { name: /compact/i })).toBeInTheDocument();
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(editor).toHaveValue("/compact ");
    });

    it("offers commands for a slash typed part-way through a draft", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "available_commands_update",
                availableCommands: [{ name: "compact", description: "Compact context", input: { hint: "focus" } }],
            },
        });
        await act(async () => window.requestAnimationFrame(() => {}));

        const editor = screen.getByRole("textbox", { name: "Message agent" });
        fireEvent.change(editor, { target: { value: "tidy up then /comp", selectionStart: 18 } });
        expect(await screen.findByRole("option", { name: /compact/i })).toBeInTheDocument();
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(editor).toHaveValue("tidy up then /compact ");
    });

    it("leaves what follows the caret in place when a command is chosen", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "available_commands_update",
                availableCommands: [{ name: "compact", description: "Compact context", input: { hint: "focus" } }],
            },
        });
        await act(async () => window.requestAnimationFrame(() => {}));

        const editor = screen.getByRole("textbox", { name: "Message agent" });
        fireEvent.change(editor, { target: { value: "run /comp then stop", selectionStart: 9 } });
        expect(await screen.findByRole("option", { name: /compact/i })).toBeInTheDocument();
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(editor).toHaveValue("run /compact then stop");
    });

    it("keeps a slash inside a word from opening the command menu", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "available_commands_update",
                availableCommands: [{ name: "compact", description: "Compact context", input: { hint: "focus" } }],
            },
        });
        await act(async () => window.requestAnimationFrame(() => {}));

        const editor = screen.getByRole("textbox", { name: "Message agent" });
        fireEvent.change(editor, { target: { value: "src/comp", selectionStart: 8 } });
        await act(async () => window.requestAnimationFrame(() => {}));
        expect(screen.queryByRole("option", { name: /compact/i })).not.toBeInTheDocument();
    });

    it("stays pinned while a restored transcript settles, and lets go when the reader scrolls up", async () => {
        render(<AgentChatPane agent={{ ...agent, resumeId: "old-session" }} cwd="/repo" active visible onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Earlier question" } },
        });
        emit("ready", { capabilities: {}, setup: {} });

        const scroller = document.querySelector(".chat-scroll") as HTMLElement;
        const view = fakeScroller(scroller, 400);
        view.scrollTo(600);
        expect(screen.queryByRole("button", { name: "Jump to latest message" })).not.toBeInTheDocument();

        // Rows measuring taller than their estimate push the bottom away. The
        // reader has not moved, so the transcript must not come unstuck.
        view.grow(3000);
        expect(screen.queryByRole("button", { name: "Jump to latest message" })).not.toBeInTheDocument();
        expect(scroller.scrollTop).toBe(2600);

        // Settling also moves the scroller itself, which must not read as the
        // reader leaving — that left old sessions stranded mid-transcript.
        view.driftTo(2200);
        expect(screen.queryByRole("button", { name: "Jump to latest message" })).not.toBeInTheDocument();
        view.grow(3600);
        expect(scroller.scrollTop).toBe(3200);

        view.scrollTo(200);
        expect(await screen.findByRole("button", { name: "Jump to latest message" })).toBeInTheDocument();
    });

    it("focuses the composer once a chat connects, and again when a hidden one is reopened", async () => {
        const props = { agent, cwd: "/repo", active: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} visible />);
        const editor = screen.getByRole("textbox", { name: "Message agent" }) as HTMLTextAreaElement;
        expect(editor.placeholder).toBe("Connecting to agent session…");

        await waitFor(() => expect(editor.placeholder).toContain("Ask about this project"));
        await nextFrame();
        expect(editor).toHaveFocus();

        rerender(<AgentChatPane {...props} visible={false} />);
        act(() => editor.blur());
        rerender(<AgentChatPane {...props} visible />);
        await nextFrame();
        expect(editor).toHaveFocus();
    });

    it("leaves a field being typed in alone when a chat connects behind it", async () => {
        const elsewhere = document.createElement("input");
        document.body.append(elsewhere);
        elsewhere.focus();
        render(<AgentChatPane agent={agent} cwd="/repo" active visible onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());

        emit("ready", { capabilities: {}, setup: {} });
        await nextFrame();
        expect(elsewhere).toHaveFocus();
        elsewhere.remove();
    });

    it("routes native path drops into prompt attachments", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });

        const editor = screen.getByRole("textbox", { name: "Message agent" });
        expect(dispatchPathDrop(editor, ["/repo/src/App.tsx"])).toBe(true);
        expect(await screen.findByText("App.tsx")).toBeInTheDocument();

        fireEvent.change(editor, { target: { value: "Review this" } });
        fireEvent.click(screen.getByRole("button", { name: "Send message" }));
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "Review this", ["/repo/src/App.tsx"]));
    });
});
