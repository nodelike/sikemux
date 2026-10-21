import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { acpApi, type AcpEvent } from "../api/acp";
import { IS_MACOS } from "../lib/platform";
import { dispatchPathDrop } from "../state/dropRegistry";
import type { Agent } from "../state/types";
import { AgentChatPane } from "./AgentChatPane";
import { shownImage } from "../state/imageViewer";
import { forgetPathState } from "./pathExistence";
import { draftOf, draftSelection, selectDraft, typeDraft } from "../test/draft";

const mocks = vi.hoisted(() => ({
    eventListener: null as ((event: AcpEvent) => void) | null,
    prompt: vi.fn(async () => {}),
    retryFailed: vi.fn(async () => {}),
    discardFailed: vi.fn(async () => {}),
    edit: vi.fn(async () => {}),
    steer: vi.fn(async () => "injected"),
    steerQueued: vi.fn(async () => {}),
    dropQueued: vi.fn(async () => {}),
    setPermissionMode: vi.fn(async () => {}),
    stopTask: vi.fn(async () => {}),
    setConfig: vi.fn(),
    setAgentModelPreferences: vi.fn(),
    setAgentPermissionMode: vi.fn(),
    /* Stands for the core starting the chat; the pane takes it up by attaching once. */
    start: vi.fn(async (_agentId: string) => ({ sessionId: "session-1", capabilities: {}, setup: {} })),
    started: null as { sessionId: string; capabilities: object; setup: object } | null,
    pathKinds: vi.fn(async (paths: string[]): Promise<(string | null)[]> => paths.map(() => null)),
    revealInFinder: vi.fn(async () => {}),
    requestOpenFile: vi.fn(),
    openUrlOnDesk: vi.fn(),
    openFileOnDesk: vi.fn(),
}));

vi.mock("../api/agents", () => ({
    agentApi: { available: async () => [], retryFailed: mocks.retryFailed, discardFailed: mocks.discardFailed },
}));

vi.mock("../api/fs", () => {
    const fsapi = {
        pathKinds: mocks.pathKinds,
        revealInFinder: mocks.revealInFinder,
        previewFile: vi.fn(async () => {
            throw new Error("no file");
        }),
    };
    return { fsapi, filesOn: () => fsapi };
});

vi.mock("../api/acp", () => ({
    acpApi: {
        subscribe: vi.fn(async (listener: (event: AcpEvent) => void) => {
            mocks.eventListener = listener;
            return () => {
                mocks.eventListener = null;
            };
        }),
        attach: vi.fn(async () => {
            const start = mocks.started;
            mocks.started = null;
            return start
                ? { status: "live", start, permissionMode: "workspace-write", running: false, turned: false, feed: "feed-1", olderBefore: null }
                : { status: "missing" };
        }),
        startChat: vi.fn(async (agentId: string) => {
            mocks.started = await mocks.start(agentId);
        }),
        setPermissionMode: mocks.setPermissionMode,
        setConfig: mocks.setConfig,
        stop: vi.fn(async () => {}),
        prompt: mocks.prompt,
        edit: mocks.edit,
        steer: mocks.steer,
        steerQueued: mocks.steerQueued,
        dropQueued: mocks.dropQueued,
        cancel: vi.fn(async () => {}),
        stopTask: mocks.stopTask,
        permissionReply: vi.fn(async () => {}),
    },
}));

vi.mock("../state/workspaceSent", () => ({ workspaceSent: async () => {} }));

vi.mock("../state/commands", () => ({
    requestOpenFile: mocks.requestOpenFile,
    openUrlOnDesk: mocks.openUrlOnDesk,
    openFileOnDesk: mocks.openFileOnDesk,
    setAgentPermissionMode: mocks.setAgentPermissionMode,
    setAgentModelPreferences: mocks.setAgentModelPreferences,
    setAgentTitle: vi.fn(),
    titleAgentFromPrompt: vi.fn(),
    toggleAgentSkipPermissions: vi.fn(),
}));

const agent: Agent = {
    id: "agent-1",
    type: "codex",
    title: "Agent session",
    permissionMode: "workspace-write",
    launchState: "live",
};

const shortcutKey = { key: "Enter", modifier: IS_MACOS ? { metaKey: true } : { ctrlKey: true } };

function emit(kind: AcpEvent["kind"], payload: Record<string, unknown>): void {
    // Streamed updates arrive a frame's worth at a time.
    act(() => mocks.eventListener?.({ agentId: agent.id, kind, payload: kind === "session_update" ? { updates: [payload] } : payload }));
}

/** The host's queue of messages waiting behind the running turn. */
function queue(...texts: string[]): void {
    emit("session_update", {
        sessionId: "session-1",
        update: { sessionUpdate: "queued_messages", messages: texts.map((text, index) => ({ id: `q${index + 1}`, text, paths: [], context: [] })) },
    });
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

function reportResize(target: Element, height?: number) {
    const borderBoxSize = height === undefined ? undefined : [{ blockSize: height, inlineSize: 0 }];
    const entries = [{ target, borderBoxSize } as unknown as ResizeObserverEntry];
    const observer = {} as ResizeObserver;
    act(() => resizeCallbacks.get(target)?.forEach((callback) => callback(entries, observer)));
}

function fakeScroller(element: HTMLElement, clientHeight: number) {
    let scrollTop = 0;
    let scrollHeight = clientHeight;
    let rowHeight = 0;
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
        // The last row measuring taller, which the list reports as it places it.
        grow(height: number) {
            const rows = element.querySelectorAll(".chat-virtual-row");
            const row = rows[rows.length - 1];
            rowHeight += height - scrollHeight;
            scrollHeight = height;
            if (row) reportResize(row, rowHeight);
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
    mocks.started = null;
    resizeCallbacks.clear();
    globalThis.ResizeObserver = TestResizeObserver as unknown as typeof ResizeObserver;
});

afterEach(cleanup);

/* The virtualizer keeps a row out of the DOM until the scroller has a size,
   and jsdom measures everything as nothing. */
async function openTranscript(prompt = "Look at the styles"): Promise<void> {
    render(<AgentChatPane agent={{ ...agent, model: "gpt-6-astra" }} cwd="/repo" active visible onBusyChange={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message agent" });
    await waitFor(() => expect(editor.getAttribute("aria-placeholder")).toContain("Ask about this project"));
    typeDraft(editor, prompt);
    fireEvent.keyDown(editor, { key: "Enter" });
    const scroller = document.querySelector(".chat-scroll") as HTMLElement;
    fakeScroller(scroller, 400);
    Object.defineProperty(scroller, "offsetWidth", { configurable: true, get: () => 600 });
    Object.defineProperty(scroller, "offsetHeight", { configurable: true, get: () => 400 });
    reportResize(scroller);
    await screen.findByRole("status");
}

async function openWithCompactCommand(): Promise<HTMLElement> {
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
    return screen.getByRole("textbox", { name: "Message agent" });
}

function findToolRow(selector: string): Promise<HTMLElement> {
    return waitFor(() => {
        const row = document.querySelector<HTMLElement>(selector);
        expect(row).not.toBeNull();
        return row as HTMLElement;
    });
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

    it("keeps the model menu's focus from a refocus queued before it opened", async () => {
        const frames = new Map<number, FrameRequestCallback>();
        let nextFrame = 0;
        const request = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
            frames.set(++nextFrame, callback);
            return nextFrame;
        });
        const cancel = vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => void frames.delete(id));
        try {
            render(<AgentChatPane agent={agent} cwd="/repo" active visible onBusyChange={() => {}} />);
            await waitFor(() => expect(screen.getByRole("button", { name: "Model" })).toBeEnabled());
            fireEvent.click(screen.getByRole("button", { name: "Model" }));
            const search = screen.getByRole("combobox", { name: "Search model" });
            expect(search).toHaveFocus();

            act(() => {
                const queued = [...frames.values()];
                frames.clear();
                queued.forEach((callback) => callback(performance.now()));
            });

            expect(search).toHaveFocus();
        } finally {
            request.mockRestore();
            cancel.mockRestore();
        }
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
            {
                id: "reasoning_effort",
                name: "Effort",
                category: "thought_level",
                type: "select",
                currentValue: "high",
                options: [{ value: "high", name: "High" }],
            },
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

    it("does not start the chat again for equivalent profile arrays", async () => {
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
        await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(1));
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

    it("says plainly when the network cut a turn short, and retries or lets go", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active visible onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "Check the build");
        fireEvent.keyDown(editor, { key: "Enter" });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledTimes(1));

        emit("turn_completed", { stopReason: "end_turn" });
        emit("error", {
            message: "Lost the connection to the model provider",
            failure: { kind: "network", title: "Lost the connection to the model provider", details: "API Error: Connection error.", account: null },
        });
        expect(await screen.findByText(/Lost connection to OpenAI/)).toBeInTheDocument();
        await new Promise((resolve) => setTimeout(resolve, 30));
        expect(mocks.prompt).toHaveBeenCalledTimes(1);

        fireEvent.click(screen.getByRole("button", { name: "Details" }));
        expect(screen.getByText("API Error: Connection error.")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Retry now" }));
        expect(mocks.retryFailed).toHaveBeenCalledWith(agent.id);

        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discard" })));
        expect(mocks.discardFailed).toHaveBeenCalledWith(agent.id);
        expect(screen.queryByText(/Lost connection to OpenAI/)).not.toBeInTheDocument();
    });

    it("stops the running turn on Escape only once the box is empty", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        typeDraft(editor, "and then the tests");

        fireEvent.keyDown(editor, { key: "Escape" });
        expect(acpApi.cancel).not.toHaveBeenCalled();
        expect(draftOf(editor)).toBe("and then the tests");

        typeDraft(editor, "");
        fireEvent.keyDown(editor, { key: "Escape", shiftKey: true });
        expect(acpApi.cancel).not.toHaveBeenCalled();
        fireEvent.keyDown(editor, { key: "Escape" });
        expect(acpApi.cancel).toHaveBeenCalledWith(agent.id);
    });

    it("lets go of selected text on Escape and leaves the running turn alone", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        typeDraft(editor, "check the login page");
        selectDraft(editor, 10, 15);

        fireEvent.keyDown(editor, { key: "Escape" });

        expect(acpApi.cancel).not.toHaveBeenCalled();
        expect(draftSelection(editor)).toEqual([15, 15]);
        expect(draftOf(editor)).toBe("check the login page");
    });

    it("leaves Escape alone while the agent is idle", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());

        fireEvent.keyDown(editor, { key: "Escape" });

        expect(acpApi.cancel).not.toHaveBeenCalled();
    });

    it("leaves Escape pressed anywhere but the composer to whatever has focus", async () => {
        render(
            <>
                <input aria-label="A form in the browser" />
                <AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />
            </>,
        );
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });

        const form = screen.getByRole("textbox", { name: "A form in the browser" });
        form.focus();
        fireEvent.keyDown(form, { key: "Escape" });
        fireEvent.keyDown(window, { key: "Escape" });

        expect(acpApi.cancel).not.toHaveBeenCalled();
    });

    it("hands a message written mid-turn to the host's queue and shows what waits there", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        typeDraft(editor, "Then look at the tests");
        fireEvent.keyDown(editor, { key: "Enter" });

        await waitFor(() => expect(mocks.prompt).toHaveBeenLastCalledWith(agent.id, "Then look at the tests", [], [], undefined));
        expect(mocks.steer).not.toHaveBeenCalled();
        queue("Then look at the tests");
        expect(await screen.findByLabelText("1 queued")).toHaveTextContent("Then look at the tests");
        expect(screen.getByRole("button", { name: "Stop agent" })).toBeInTheDocument();

        queue();
        await waitFor(() => expect(screen.queryByLabelText("1 queued")).not.toBeInTheDocument());
    });

    it("writes a sent message again and takes the chat back to before it", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { editing: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const composer = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(composer).toBeEnabled());
        typeDraft(composer, "First try");
        fireEvent.keyDown(composer, { key: "Enter" });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledTimes(1));
        const messageId = (mocks.prompt.mock.calls[0] as unknown[])[4];
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "agent_message_chunk", messageId: "a1", content: { type: "text", text: "The old answer" } },
        });
        emit("turn_completed", {});

        fireEvent.click(await screen.findByRole("button", { name: "Edit message" }));
        const field = screen.getByRole("textbox", { name: "Edit message" });
        fireEvent.change(field, { target: { value: "Second try" } });
        fireEvent.keyDown(field, { key: "Enter" });

        await waitFor(() => expect(mocks.edit).toHaveBeenCalledWith(agent.id, messageId, "Second try", [], []));
        expect(screen.queryByText("The old answer")).not.toBeInTheDocument();
        expect(screen.queryByText("First try")).not.toBeInTheDocument();
        expect(screen.getByText("Second try")).toBeInTheDocument();
    });

    it("offers no edit while the agent cannot take one back", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const composer = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(composer).toBeEnabled());
        typeDraft(composer, "Hello");
        fireEvent.keyDown(composer, { key: "Enter" });
        emit("turn_completed", {});
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalled());
        expect(screen.queryByRole("button", { name: "Edit message" })).not.toBeInTheDocument();
    });

    it("brings back sent messages with the arrow keys from an empty composer", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        typeDraft(editor, "Then look at the tests");
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(draftOf(editor)).toBe("");
        queue("Then look at the tests");
        await screen.findByLabelText("1 queued");

        fireEvent.keyDown(editor, { key: "ArrowUp" });
        expect(draftOf(editor)).toBe("Then look at the tests");
        fireEvent.keyDown(editor, { key: "ArrowUp" });
        expect(draftOf(editor)).toBe("First");
        fireEvent.keyDown(editor, { key: "ArrowUp" });
        expect(draftOf(editor)).toBe("First");

        fireEvent.keyDown(editor, { key: "ArrowDown" });
        expect(draftOf(editor)).toBe("Then look at the tests");
        fireEvent.keyDown(editor, { key: "ArrowDown" });
        expect(draftOf(editor)).toBe("");
    });

    it("leaves the arrows to the text being written", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        typeDraft(editor, "one line that wraps across the box");
        selectDraft(editor, 3, 3);

        expect(fireEvent.keyDown(editor, { key: "ArrowUp" })).toBe(true);
        expect(draftOf(editor)).toBe("one line that wraps across the box");

        typeDraft(editor, "");
        fireEvent.keyDown(editor, { key: "ArrowUp" });
        expect(draftOf(editor)).toBe("First");
        typeDraft(editor, "First, but better");
        fireEvent.keyDown(editor, { key: "ArrowUp" });
        expect(draftOf(editor)).toBe("First, but better");
    });

    it("offers no steering for an agent that cannot take a message mid-turn", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        typeDraft(editor, "Second");
        fireEvent.keyDown(editor, { key: shortcutKey.key, ...shortcutKey.modifier });
        queue("Second");

        expect(await screen.findByLabelText("1 queued")).toHaveTextContent("Second");
        expect(mocks.steerQueued).not.toHaveBeenCalled();
    });

    it("puts a queued message into the running turn when its steer is asked for", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        queue("Actually, check the other file");

        fireEvent.click(await screen.findByRole("button", { name: "Steer the running turn with Actually, check the other file" }));

        await waitFor(() => expect(mocks.steerQueued).toHaveBeenCalledWith(agent.id, ["q1"], undefined));
    });

    it("steers straight from the composer on Enter, with whatever waits in the queue", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        queue("Then the tests");
        await screen.findByLabelText("1 queued");

        typeDraft(editor, "And the rail");
        fireEvent.keyDown(editor, { key: "Enter" });

        await waitFor(() => expect(mocks.steerQueued).toHaveBeenCalledWith(agent.id, ["q1"], { text: "And the rail", paths: [], context: [] }));
        expect(mocks.prompt).toHaveBeenCalledTimes(1);
    });

    it("steers every queued message, from the queue's own button or the shortcut", async () => {
        mocks.start.mockResolvedValueOnce({ sessionId: "session-1", capabilities: { steering: true }, setup: {} });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "First");
        fireEvent.keyDown(editor, { key: "Enter" });
        queue("Then the tests", "And the rail");

        fireEvent.click(await screen.findByRole("button", { name: "Steer the running turn with every queued message" }));
        await waitFor(() => expect(mocks.steerQueued).toHaveBeenCalledWith(agent.id, ["q1", "q2"], undefined));

        fireEvent.keyDown(editor, { key: shortcutKey.key, ...shortcutKey.modifier });
        await waitFor(() => expect(mocks.steerQueued).toHaveBeenCalledTimes(2));
    });

    it("keeps saying the turn is alive, and names the tool it is running", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "Check the tests");
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(await screen.findByRole("status")).toHaveTextContent("Thinking…");

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "mcp__github__list_issues", status: "in_progress" },
        });
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("list issues"));

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

    it("says what a running tool is doing instead of quoting the command it was given", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "Check the styles");
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

    it("opens the page a fetch names from its row", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-1",
                kind: "fetch",
                title: "Fetch https://docs.livekit.io/home/self-hosting/deployment/",
                status: "in_progress",
            },
        });

        const link = await screen.findByRole("link", { name: "https://docs.livekit.io/home/self-hosting/deployment/" });
        fireEvent.click(link);
        expect(mocks.openUrlOnDesk).toHaveBeenCalledWith(agent.id, "https://docs.livekit.io/home/self-hosting/deployment/");
    });

    it("finds text across the conversation with the find shortcut", async () => {
        await openTranscript("Look at the styles");
        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "The styles are in chat.css" } },
        });
        await screen.findByText("The styles are in chat.css");

        fireEvent.keyDown(window, { key: "f", code: "KeyF", metaKey: IS_MACOS, ctrlKey: !IS_MACOS });
        const input = await screen.findByRole("textbox", { name: "Find in chat" });
        expect(input).toHaveFocus();

        fireEvent.change(input, { target: { value: "styles" } });
        expect(screen.getByText("1/2")).toBeInTheDocument();
        fireEvent.keyDown(input, { key: "Enter" });
        expect(screen.getByText("2/2")).toBeInTheDocument();
        fireEvent.keyDown(input, { key: "Enter" });
        expect(screen.getByText("1/2")).toBeInTheDocument();
        fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
        expect(screen.getByText("2/2")).toBeInTheDocument();

        fireEvent.change(input, { target: { value: "nowhere" } });
        expect(screen.getByText("No results")).toBeInTheDocument();

        fireEvent.keyDown(input, { key: "Escape" });
        expect(screen.queryByRole("textbox", { name: "Find in chat" })).toBeNull();
        expect(screen.getByRole("textbox", { name: "Message agent" })).toHaveFocus();
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

        const rows = await waitFor(() => {
            const found = [...document.querySelectorAll<HTMLElement>(".chat-tool")];
            expect(found).toHaveLength(2);
            return found;
        });
        expect(rows[0]).toHaveTextContent("run");
        expect(rows[0]).toHaveTextContent("pnpm vitest run src/lib/shaderField.test.ts");
        expect(rows[1]).toHaveTextContent("BrowserPane.tsx");
        expect(rows[1]).not.toHaveTextContent("src/components");
    });

    it("puts the file a call touched on the desk, opens it in the editor on a double click, and still opens what the call did", async () => {
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
        expect(mocks.openFileOnDesk).toHaveBeenCalledWith(agent.id, "/repo/src/styles/stage.css", 1, undefined);
        expect(mocks.requestOpenFile).not.toHaveBeenCalled();
        fireEvent.doubleClick(file);
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/repo/src/styles/stage.css", 1, undefined, undefined);

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

        const edit = await findToolRow(".chat-tool[data-kind='edit']");
        expect(edit).toHaveTextContent("+1");
        expect(edit).toHaveTextContent("−1");
        expect(screen.queryByText(/background: transparent;/)).not.toBeInTheDocument();

        fireEvent.click(edit);
        const added = await waitFor(() => document.querySelector(".chat-diff-line.add") as HTMLElement);
        expect(added).toHaveTextContent("background: transparent;");
        // Only the part that changed is marked, not the whole line.
        expect(added.querySelector("mark")).toHaveTextContent("transparent");
        expect(document.querySelector(".chat-diff-line.del")).toHaveTextContent("background: var(--pane);");

        fireEvent.click(document.querySelector(".chat-tool.status-failed") as HTMLElement);
        expect(await screen.findByText(/1 failed/)).toBeInTheDocument();
    });

    it("says what a running command is for while it runs, and counts the run again once it ends", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-1",
                kind: "execute",
                title: "pnpm build",
                status: "in_progress",
                rawInput: { command: "pnpm build", description: "Build the site" },
            },
        });

        const header = await screen.findByRole("button", { name: /Build the site/ });
        expect(header).toHaveClass("live");
        expect(document.querySelector(".chat-tool.live")).not.toBeNull();

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "completed", rawOutput: "built in 2.9s" },
        });
        const finished = await screen.findByRole("button", { name: /1 tool call/ });
        expect(finished).not.toHaveClass("live");
        expect(document.querySelector(".chat-tool.live")).toBeNull();
    });

    it("opens a command onto the whole of it and what it printed, folding a long output", async () => {
        await openTranscript();
        const printed = Array.from({ length: 20 }, (_, index) => `line ${index + 1}`).join("\n");
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "tool_call",
                toolCallId: "tool-1",
                kind: "execute",
                title: "python3 - <<'EOF'\nprint('hi')\nEOF",
                status: "completed",
                rawOutput: printed,
            },
        });

        const row = await findToolRow(".chat-tool");
        expect(row).toHaveTextContent("python3 - <<'EOF'");
        expect(row).not.toHaveTextContent("print('hi')");

        fireEvent.click(row);
        const terminal = await waitFor(() => document.querySelector(".chat-tool-terminal") as HTMLElement);
        expect(terminal.querySelector(".chat-tool-command pre")).toHaveTextContent("print('hi')");
        expect(screen.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Copy output" })).toBeInTheDocument();
        expect(terminal.querySelector(".chat-tool-output pre")).toHaveTextContent("line 12");
        expect(terminal.querySelector(".chat-tool-output pre")).not.toHaveTextContent("line 13");

        fireEvent.click(screen.getByRole("button", { name: "Show all 20 lines" }));
        expect(terminal.querySelector(".chat-tool-output pre")).toHaveTextContent("line 20");
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

    it("draws an answer as it streams: a fence still open, then the table after it", async () => {
        await openTranscript();
        const chunk = (text: string) =>
            emit("session_update", { sessionId: "session-1", update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });

        chunk("Here is the fix:\n\n```ts\nconst a = 1;\n");
        const code = await waitFor(() => {
            const found = document.querySelector(".chat-markdown pre code.language-ts");
            expect(found).toHaveTextContent("const a = 1;");
            return found as HTMLElement;
        });
        expect(code.closest("pre")!.querySelector(".chat-code-title")).not.toBeNull();
        const intro = screen.getByText("Here is the fix:");

        chunk("```\n\n| approach | cpu |\n| --- | --- |\n| batch | 12% |\n");
        expect((await screen.findByText("batch")).tagName).toBe("TD");
        expect(screen.getByText("Here is the fix:")).toBe(intro);

        emit("turn_completed", { stopReason: "end_turn" });
        await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
        expect(screen.getByText("Here is the fix:")).toBe(intro);
        expect(document.querySelectorAll(".chat-markdown pre")).toHaveLength(1);
    });

    it("titles a fence whose name is not valid percent-encoding with the name as written", async () => {
        await openTranscript();
        emit("session_update", {
            sessionId: "session-1",
            update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "```100%\nfull\n```\n" },
            },
        });

        await waitFor(() => expect(document.querySelector(".chat-code-title")).toHaveTextContent("100%"));
    });

    it("keeps a subagent over the composer past a cancelled turn and settles its card when it stops", async () => {
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
        expect(screen.getByLabelText("1 subagent")).toBeInTheDocument();

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "subagent_state_update", subagentSessionId: "subagent-1", state: "cancelled" },
        });

        await waitFor(() => expect(screen.queryByLabelText("1 subagent")).not.toBeInTheDocument());
        expect(document.querySelector('.chat-subagent [role="img"][aria-label="stopped"]')).not.toBeNull();
        expect(document.querySelector(".chat-tool-spinner")).toBeNull();
    });

    it("shows the agent starting and waits to be asked before starting a failed agent again", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());

        emit("status", { state: "starting" });
        expect(document.querySelector(".chat-welcome")).not.toBeNull();
        emit("status", { state: "error", reason: "failed", message: "agent failed" });
        emit("error", { message: "agent failed" });
        expect(screen.getByText("Structured session unavailable.")).toBeInTheDocument();
        expect(mocks.start).toHaveBeenCalledTimes(1);

        fireEvent.click(screen.getByRole("button", { name: "Reconnect" }));
        await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(2));
    });

    describe("an agent that dies under the chat", () => {
        const saved = { ...agent, resumeId: "session-1" };

        async function openReady(chatAgent: Agent) {
            render(<AgentChatPane agent={chatAgent} cwd="/repo" active visible profile={undefined} onBusyChange={() => {}} />);
            await waitFor(() => expect(mocks.eventListener).not.toBeNull());
            await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(1));
            emit("ready", { capabilities: {}, setup: {} });
        }

        it("shows one the core gave up starting again, with the detail and a retry", async () => {
            let resumed!: () => void;
            mocks.start.mockImplementationOnce(async () => ({ sessionId: "session-1", capabilities: {}, setup: {} }));
            mocks.start.mockImplementationOnce(
                () =>
                    new Promise((resolve) => {
                        resumed = () => resolve({ sessionId: "session-1", capabilities: {}, setup: {} });
                    }),
            );
            await openReady(saved);
            emit("status", { state: "error", reason: "exited", message: "Process exited with 1: out of memory" });
            expect(screen.getByText("Couldn't resume this chat")).toBeInTheDocument();
            expect(screen.getByText("Process exited with 1: out of memory")).toHaveClass("chat-recovery-detail");
            await act(async () => {});
            expect(mocks.start).toHaveBeenCalledTimes(1);

            fireEvent.click(screen.getByRole("button", { name: "Retry" }));
            await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(2));
            expect(screen.getByText("Resuming…")).toBeInTheDocument();

            await act(async () => resumed());
            await waitFor(() => expect(screen.queryByText("Resuming…")).not.toBeInTheDocument());
        });

        it("never offers to resume a stop that was asked for", async () => {
            await openReady(saved);
            emit("status", { state: "stopped", reason: "requested" });
            await act(async () => {});
            expect(mocks.start).toHaveBeenCalledTimes(1);
            expect(screen.queryByText("Couldn't resume this chat")).not.toBeInTheDocument();
        });

        it("says so when a retry fails to start", async () => {
            mocks.start.mockImplementationOnce(async () => ({ sessionId: "session-1", capabilities: {}, setup: {} }));
            mocks.start.mockImplementationOnce(async () => {
                throw new Error("session not found");
            });
            await openReady(saved);
            emit("status", { state: "stopped", reason: "exited" });
            fireEvent.click(screen.getByRole("button", { name: "Retry" }));

            expect(await screen.findByText("session not found")).toBeInTheDocument();
            expect(screen.getByText("Couldn't resume this chat")).toBeInTheDocument();
            expect(mocks.start).toHaveBeenCalledTimes(2);
        });

        it("cannot resume a chat with no saved session, and retries with a fresh one", async () => {
            await openReady(agent);
            emit("status", { state: "stopped", reason: "exited" });
            expect(screen.getByText("Couldn't resume this chat")).toBeInTheDocument();
            await act(async () => {});
            expect(mocks.start).toHaveBeenCalledTimes(1);

            fireEvent.click(screen.getByRole("button", { name: "Retry" }));
            await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(2));
        });

        it("sends a message written while it resumes once it is back", async () => {
            await openReady(saved);
            emit("status", { state: "stopped", reason: "exited" });
            fireEvent.click(screen.getByRole("button", { name: "Retry" }));

            const editor = screen.getByRole("textbox", { name: "Message agent" });
            typeDraft(editor, "carry on");
            fireEvent.keyDown(editor, { key: "Enter" });

            await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "carry on", [], [], expect.any(String)));
        });
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
        await waitFor(() => expect(screen.queryByRole("button", { name: "Stop pnpm test" })).not.toBeInTheDocument());
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
        const editor = await openWithCompactCommand();
        typeDraft(editor, "/");
        expect(await screen.findByRole("option", { name: /compact/i })).toBeInTheDocument();
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(draftOf(editor)).toBe("/compact ");
    });

    it("leaves what follows the caret in place when a command is chosen", async () => {
        const editor = await openWithCompactCommand();
        typeDraft(editor, "run /comp then stop", 9);
        expect(await screen.findByRole("option", { name: /compact/i })).toBeInTheDocument();
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(draftOf(editor)).toBe("run /compact then stop");
    });

    it("keeps a slash inside a word from opening the command menu", async () => {
        const editor = await openWithCompactCommand();
        typeDraft(editor, "src/comp", 8);
        await act(async () => window.requestAnimationFrame(() => {}));
        expect(screen.queryByRole("option", { name: /compact/i })).not.toBeInTheDocument();
    });

    /* The transcript is laid out from the bottom, so its scroll position is 0
       at the bottom and negative above it, and the browser holds the bottom
       still by itself. */
    it("rests on the bottom, and lets go when the reader scrolls up", async () => {
        await openTranscript();
        const scroller = document.querySelector(".chat-scroll") as HTMLElement;
        expect(screen.queryByRole("button", { name: "Jump to latest message" })).not.toBeInTheDocument();

        scroller.scrollTop = -40;
        fireEvent.scroll(scroller);
        expect(screen.queryByRole("button", { name: "Jump to latest message" })).not.toBeInTheDocument();

        scroller.scrollTop = -600;
        fireEvent.scroll(scroller);
        fireEvent.click(await screen.findByRole("button", { name: "Jump to latest message" }));
        expect(scroller.scrollTop).toBe(0);
        expect(screen.queryByRole("button", { name: "Jump to latest message" })).not.toBeInTheDocument();
    });

    it("keeps the reader's place when something grows below them, and not when the row they are on draws", async () => {
        await openTranscript();
        const scroller = document.querySelector(".chat-scroll") as HTMLElement;
        const content = document.querySelector(".chat-scroll-content") as HTMLElement;
        const row = document.querySelector(".chat-row") as HTMLElement;
        const reply = row.cloneNode() as HTMLElement;
        row.after(reply);
        let box = { top: -40, bottom: 120 };
        scroller.getBoundingClientRect = () => ({ top: 0, left: 0, width: 600, height: 400, bottom: 400, right: 600 }) as DOMRect;
        row.getBoundingClientRect = () => box as DOMRect;
        Object.defineProperty(document, "elementFromPoint", { configurable: true, value: () => row });

        scroller.scrollTop = -900;
        fireEvent.scroll(scroller);
        // The row at the top of the view is drawn at its real height, which it grows into upwards.
        box = { top: -400, bottom: 120 };
        reportResize(content);
        expect(scroller.scrollTop).toBe(-900);

        // A reply streaming in below pushes the row the reader is on up the screen.
        box = { top: -600, bottom: -80 };
        reportResize(content);
        expect(scroller.scrollTop).toBe(-1100);
        reply.remove();
        Reflect.deleteProperty(document, "elementFromPoint");
    });

    it("shows a resumed chat's history once it is all in", async () => {
        let answer: (response: { sessionId: string; capabilities: object; setup: object }) => void = () => {};
        mocks.start.mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
        render(<AgentChatPane agent={{ ...agent, resumeId: "old-session" }} cwd="/repo" active visible onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.start).toHaveBeenCalled());

        emit("session_update", {
            sessionId: "session-1",
            update: { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Earlier question" } },
        });
        await nextFrame();
        expect(screen.queryByText("Earlier question")).not.toBeInTheDocument();

        await act(async () => answer({ sessionId: "session-1", capabilities: {}, setup: {} }));
        expect(await screen.findByText("Earlier question")).toBeInTheDocument();
    });

    it("focuses the composer once a chat connects, and again when a hidden one is reopened", async () => {
        const props = { agent, cwd: "/repo", active: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} visible />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        expect(editor.getAttribute("aria-placeholder")).toContain("Ask about this project");

        await waitFor(() => expect(editor.getAttribute("aria-placeholder")).toContain("Ask about this project"));
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
        expect(await screen.findByRole("button", { name: "Remove App.tsx" })).toBeInTheDocument();

        typeDraft(editor, "Review this");
        fireEvent.click(screen.getByRole("button", { name: "Send message" }));
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "Review this", ["/repo/src/App.tsx"], [], expect.any(String)));
    });
});
