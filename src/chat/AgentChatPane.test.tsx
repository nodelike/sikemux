import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { acpApi, type AcpAttachment, type AcpEvent } from "../api/acp";
import type { ChatHead } from "../api/generated/wire";
import { IS_MACOS } from "../lib/platform";
import { dispatchPathDrop } from "../state/dropRegistry";
import type { Agent } from "../state/types";
import { AgentChatPane } from "./AgentChatPane";
import { shownImage } from "../state/imageViewer";
import { forgetPathState } from "./pathExistence";
import type { ChatChange, ChatMessage, ChatPart, ChatView, SubagentCard, ToolCall, ToolDetail } from "./types";
import { draftOf, draftSelection, selectDraft, typeDraft } from "../test/draft";

const mocks = vi.hoisted(() => ({
    eventListener: null as ((event: AcpEvent) => void) | null,
    /* The number of the last change the core sent. */
    seq: 0,
    prompt: vi.fn(async (..._args: unknown[]) => {}),
    retryFailed: vi.fn(async () => {}),
    discardFailed: vi.fn(async () => {}),
    edit: vi.fn(async () => {}),
    steer: vi.fn(async () => "injected"),
    steerQueued: vi.fn(async () => {}),
    dropQueued: vi.fn(async () => {}),
    setPermissionMode: vi.fn(async (_agentId: string, _mode: string) => {}),
    stopTask: vi.fn(async () => {}),
    setConfig: vi.fn(),
    toolDetail: vi.fn(async (_agentId: string, _toolCallId: string): Promise<ToolDetail> => ({})),
    setAgentPermissionMode: vi.fn(),
    /* Stands for the core starting the chat, answering how its head stands; the pane takes it up by attaching once. */
    start: vi.fn(async (_agentId: string): Promise<Partial<ChatHead>> => ({})),
    started: null as { head: Partial<ChatHead>; seq: number } | null,
    /* How many times the pane has taken a live chat up. */
    attached: 0,
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
        composing: vi.fn(async () => {}),
        attach: vi.fn(async (agentId: string): Promise<AcpAttachment> => {
            let started = mocks.started;
            if (!started) {
                const seq = mocks.seq;
                started = { head: await mocks.start(agentId), seq };
            }
            mocks.started = null;
            mocks.attached += 1;
            return { status: "live", view: chatView(started.head), feed: "feed-1", seq: started.seq };
        }),
        startChat: vi.fn(async (agentId: string) => {
            const seq = mocks.seq;
            mocks.started = { head: await mocks.start(agentId), seq };
        }),
        setPermissionMode: mocks.setPermissionMode,
        setConfig: mocks.setConfig,
        toolDetail: mocks.toolDetail,
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

/* What the core sends: a view of the chat to open on, then numbered changes to it. */
function chatView(head: Partial<ChatHead> = {}, rows: ChatMessage[] = []): ChatView {
    return {
        head: {
            sessionId: "session-1",
            connection: "ready",
            running: false,
            turned: false,
            permissionMode: "workspace-write",
            commands: [],
            capabilities: {},
            setup: {},
            permissions: [],
            tasks: [],
            runningSubagents: [],
            queued: [],
            earlierLost: false,
            ...head,
        },
        rows,
    };
}

const text = (id: string, said: string): ChatPart => ({ kind: "text", id, text: said });

const toolPart = (id: string, tool: Partial<ToolCall>, summary: Partial<Extract<ChatPart, { kind: "tool" }>> = {}): ChatPart => ({
    kind: "tool",
    id,
    tool: { toolCallId: id, title: "", status: "pending", row: { action: "other", target: "" }, ...tool },
    ...summary,
});

const listIssues = { action: "mcp", target: "list issues", server: "github", activity: "list issues" } as const;
const searchPty = { action: "search", target: "usePty" } as const;

const subagentPart = (card: SubagentCard): ChatPart => ({ kind: "subagent", id: `sub-${card.sessionId}`, subagent: card });

const message = (id: string, parts: ChatPart[], role: ChatMessage["role"] = "assistant"): ChatMessage => ({ id, role, parts });

const head = (set: Partial<ChatHead> | Record<string, unknown>): ChatChange => ({ op: "head", set });
const putRow = (row: ChatMessage, within?: string): ChatChange => ({ op: "row", row, ...(within ? { in: within } : {}) });
const putPart = (row: string, part: ChatPart): ChatChange => ({ op: "part", row, part });
const append = (row: string, part: string, said: string): ChatChange => ({ op: "text", row, part, text: said });

function send(...changes: ChatChange[]): void {
    mocks.seq += 1;
    const seq = mocks.seq;
    act(() => mocks.eventListener?.({ agentId: agent.id, kind: "changes", seq, payload: changes }));
}

/** The host's queue of messages waiting behind the running turn. */
function queue(...texts: string[]): void {
    send(head({ queued: texts.map((said, index) => ({ id: `q${index + 1}`, text: said, paths: [], context: [] })) }));
}

/* The core shows the person's last message back as its own row, with the turn it started running. */
function showPromptBack(id = "u1"): void {
    const [, said, paths, , messageId] = mocks.prompt.mock.calls.at(-1) as [string, string, string[], unknown, string];
    send(
        putRow({ ...message(id, [text(`${id}.0`, said)], "user"), promptId: messageId, ...(paths.length ? { attachments: paths } : {}) }),
        head({ running: true }),
    );
}

/** Waits for the pane to have taken the chat up `times` times. */
const connected = (times = 1) => waitFor(() => expect(mocks.attached).toBe(times));

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
    /* The core takes the new mode up and says so in the chat's head. */
    mocks.setPermissionMode.mockImplementation(async (_agentId: string, mode: string) => {
        await Promise.resolve();
        send(head({ permissionMode: mode }));
    });
    mocks.eventListener = null;
    mocks.started = null;
    mocks.seq = 0;
    mocks.attached = 0;
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
    await waitFor(() => expect(mocks.prompt).toHaveBeenCalled());
    showPromptBack();
    const scroller = document.querySelector(".chat-scroll") as HTMLElement;
    fakeScroller(scroller, 400);
    Object.defineProperty(scroller, "offsetWidth", { configurable: true, get: () => 600 });
    Object.defineProperty(scroller, "offsetHeight", { configurable: true, get: () => 400 });
    reportResize(scroller);
    await screen.findByRole("status");
}

async function openWithCompactCommand(): Promise<HTMLElement> {
    render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
    await connected();
    send(head({ commands: [{ name: "compact", description: "Compact context", hint: "focus" }] }));
    await act(async () => window.requestAnimationFrame(() => {}));
    return screen.getByRole("textbox", { name: "Message agent" });
}

function findElement(selector: string): Promise<HTMLElement> {
    return waitFor(() => {
        const row = document.querySelector<HTMLElement>(selector);
        expect(row).not.toBeNull();
        return row as HTMLElement;
    });
}

const stageLines: NonNullable<ToolDetail["diff"]>["lines"] = [
    { sign: " ", text: ".stage {", oldLine: 1, newLine: 1 },
    { sign: "-", text: "    background: var(--pane);", oldLine: 2, mark: [16, 27] },
    { sign: "+", text: "    background: transparent;", newLine: 2, mark: [16, 27] },
    { sign: " ", text: "}", oldLine: 3, newLine: 3 },
];

describe("AgentChatPane", () => {
    it("keeps receiving hidden session updates while freezing transcript rendering", async () => {
        const props = { agent, cwd: "/repo", active: true, visible: true, onBusyChange: () => {} };
        const { rerender } = render(<AgentChatPane {...props} />);
        await connected();
        rerender(<AgentChatPane {...props} visible={false} />);
        send(
            head({
                permissions: [
                    {
                        requestId: "hidden-request",
                        sessionId: "session-1",
                        toolCall: { toolCallId: "tool-1", title: "Touch a file" },
                        options: [{ optionId: "allow", name: "Allow hidden tool", kind: "allow_once" }],
                    },
                ],
            }),
        );
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
        await connected();
        send(putRow(message("u0", [text("u0.0", "Existing message")], "user")));
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

    it("changes the model live without restarting the chat", async () => {
        const model = (current: string) => ({
            id: "model",
            current,
            choices: [
                { value: "astra", label: "GPT-6 Astra" },
                { value: "sol", label: "GPT-5.6 Sol" },
            ],
        });
        const effort = { id: "reasoning_effort", current: "high", choices: [{ value: "high", label: "High" }] };
        mocks.start.mockResolvedValueOnce({ model: model("astra"), effort });
        mocks.setConfig.mockResolvedValueOnce({});
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await waitFor(() => expect(screen.getByRole("button", { name: "Model" })).toBeEnabled());
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.change(screen.getByRole("combobox", { name: "Search model" }), { target: { value: "Sol" } });
        fireEvent.keyDown(screen.getByRole("combobox", { name: "Search model" }), { key: "Enter" });
        await waitFor(() => expect(mocks.setConfig).toHaveBeenCalledWith(agent.id, "model", "sol"));
        send(head({ model: model("sol") }));
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
        send(head({ permissionMode: "bypass" }));
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

        showPromptBack();
        send(
            head({
                running: false,
                stopReason: "end_turn",
                error: "Lost the connection to the model provider",
                failure: { kind: "network", details: "API Error: Connection error." },
            }),
        );
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
        mocks.start.mockResolvedValueOnce({ capabilities: { editing: true } });
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const composer = screen.getByRole("textbox", { name: "Message agent" });
        await connected();
        typeDraft(composer, "First try");
        fireEvent.keyDown(composer, { key: "Enter" });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledTimes(1));
        const messageId = (mocks.prompt.mock.calls[0] as unknown[])[4] as string;
        showPromptBack();
        send(putRow(message("a1", [text("a1.0", "The old answer")])), head({ running: false }));

        fireEvent.click(await screen.findByRole("button", { name: "Edit message" }));
        const field = screen.getByRole("textbox", { name: "Edit message" });
        fireEvent.change(field, { target: { value: "Second try" } });
        fireEvent.keyDown(field, { key: "Enter" });

        await waitFor(() => expect(mocks.edit).toHaveBeenCalledWith(agent.id, messageId, "Second try", [], []));
        expect(screen.queryByText("The old answer")).not.toBeInTheDocument();
        expect(screen.queryByText("First try")).not.toBeInTheDocument();

        send(
            { op: "drop", from: "u1" },
            putRow({ ...message("u2", [text("u2.0", "Second try")], "user"), promptId: messageId }),
            head({ running: true }),
        );
        expect(await screen.findByText("Second try")).toBeInTheDocument();
    });

    it("offers no edit while the agent cannot take one back", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const composer = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(composer).toBeEnabled());
        typeDraft(composer, "Hello");
        fireEvent.keyDown(composer, { key: "Enter" });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalled());
        showPromptBack();
        send(putRow(message("a1", [text("a1.0", "Hi")])), head({ running: false }));
        await screen.findByText("Hi");
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
        mocks.start.mockResolvedValueOnce({ capabilities: { steering: true } });
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
        mocks.start.mockResolvedValueOnce({ capabilities: { steering: true } });
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
        mocks.start.mockResolvedValueOnce({ capabilities: { steering: true } });
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
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalled());
        showPromptBack();

        send(
            putRow(message("a1", [toolPart("tool-1", { title: "mcp__github__list_issues", status: "in_progress", row: listIssues })])),
            head({ activeTool: "tool-1" }),
        );
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("list issues"));

        send(
            putPart("a1", toolPart("tool-1", { title: "mcp__github__list_issues", status: "completed", row: listIssues })),
            head({ activeTool: null }),
        );
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Thinking…"));

        send(head({ running: false, stopReason: "end_turn" }));
        await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    });

    it("reads every change in one streamed frame", async () => {
        await openTranscript();
        send(putRow(message("a1", [text("a1.0", "One ")])), append("a1", "a1.0", "two "), append("a1", "a1.0", "three"));
        expect(await screen.findByText("One two three")).toBeInTheDocument();
    });

    it("says what a running tool is doing instead of quoting the command it was given", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "Check the styles");
        fireEvent.keyDown(editor, { key: "Enter" });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalled());
        showPromptBack();

        send(
            putRow(
                message("a1", [
                    toolPart("tool-1", {
                        kind: "execute",
                        title: 'grep -n "is-transparent" -A12 src/styles/base.css | head -40',
                        status: "in_progress",
                        row: { action: "execute", target: 'grep -n "is-transparent" -A12 src/styles/base.css | head -40' },
                    }),
                ]),
            ),
            head({ activeTool: "tool-1" }),
        );
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Running a command…"));
        expect(screen.getByRole("status")).not.toHaveTextContent("is-transparent");
    });

    it("holds a run of tool calls open through the gaps between them, and folds it once the turn ends", async () => {
        await openTranscript();
        const read = (id: string, file: string, status: string) =>
            toolPart(id, { kind: "read", title: `src/styles/${file}`, status, row: { action: "read", target: file, path: `src/styles/${file}` } });
        send(
            putRow(message("a1", [{ kind: "thought", id: "a1.0", text: "Weighing the two options" }, read("tool-1", "chat.css", "in_progress")])),
            head({ activeTool: "tool-1" }),
        );

        expect(await screen.findByText("Weighing the two options")).toBeVisible();
        const run = await screen.findByRole("button", { name: /1 tool call/ });
        expect(run).toHaveAttribute("aria-expanded", "true");

        // The turn is still going, so the wait for the next call is not a fold.
        send(putPart("a1", read("tool-1", "chat.css", "completed")), head({ activeTool: null }));
        expect(run).toHaveAttribute("aria-expanded", "true");

        send(putPart("a1", read("tool-2", "base.css", "in_progress")), head({ activeTool: "tool-2" }));
        send(putPart("a1", read("tool-2", "base.css", "completed")), head({ activeTool: null }));
        const finished = await screen.findByRole("button", { name: /2 tool calls/ });
        expect(finished).toHaveAttribute("aria-expanded", "true");
        expect(document.querySelectorAll(".chat-tool")).toHaveLength(2);

        send(head({ running: false, stopReason: "end_turn" }));
        await waitFor(() => expect(finished).toHaveAttribute("aria-expanded", "false"));
        expect(document.querySelectorAll(".chat-tool")).toHaveLength(0);

        fireEvent.click(finished);
        expect(finished).toHaveAttribute("aria-expanded", "true");
        expect(document.querySelectorAll(".chat-tool")).toHaveLength(2);
    });

    it("opens the page a fetch names from its row", async () => {
        await openTranscript();
        send(
            putRow(
                message("a1", [
                    toolPart("tool-1", {
                        kind: "fetch",
                        title: "Fetch https://docs.livekit.io/home/self-hosting/deployment/",
                        status: "in_progress",
                        row: { action: "fetch", target: "Fetch https://docs.livekit.io/home/self-hosting/deployment/" },
                    }),
                ]),
            ),
            head({ activeTool: "tool-1" }),
        );

        const link = await screen.findByRole("link", { name: "https://docs.livekit.io/home/self-hosting/deployment/" });
        fireEvent.click(link);
        expect(mocks.openUrlOnDesk).toHaveBeenCalledWith(agent.id, "https://docs.livekit.io/home/self-hosting/deployment/");
    });

    it("finds text across the conversation with the find shortcut", async () => {
        await openTranscript("Look at the styles");
        send(putRow(message("a1", [text("a1.0", "The styles are in chat.css")])));
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
        const card: SubagentCard = { sessionId: "subagent-1", name: "Explorer", task: "Read the styles", state: "running", calls: 0 };
        send(
            putRow(message("a1", [subagentPart(card)])),
            putRow(message("s1", [text("s1.0", "Found the pane")]), "subagent-1"),
            head({ runningSubagents: [card] }),
        );

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
        send(
            putRow(
                message("a1", [
                    toolPart("tool-1", {
                        kind: "read",
                        title: "src/styles/chat.css",
                        status: "completed",
                        row: { action: "read", target: "chat.css", path: "src/styles/chat.css" },
                    }),
                ]),
            ),
        );
        const run = await screen.findByRole("button", { name: /1 tool call/ });
        expect(run).toHaveAttribute("aria-expanded", "true");

        send(putPart("a1", text("a1.1", "The pane paints no background")));
        await waitFor(() => expect(run).toHaveAttribute("aria-expanded", "false"));
    });

    it("hangs each call off the run as a kind, a target and how long it took", async () => {
        await openTranscript();
        send(
            putRow(
                message("a1", [
                    toolPart("tool-1", {
                        kind: "execute",
                        title: "pnpm vitest run src/lib/shaderField.test.ts",
                        status: "in_progress",
                        row: { action: "execute", target: "pnpm vitest run src/lib/shaderField.test.ts" },
                    }),
                    toolPart("tool-2", {
                        kind: "read",
                        title: "src/components/browser/BrowserPane.tsx",
                        status: "completed",
                        row: { action: "read", target: "BrowserPane.tsx", path: "src/components/browser/BrowserPane.tsx" },
                    }),
                ]),
            ),
        );

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
        mocks.toolDetail.mockResolvedValue({ diff: { path: "src/styles/stage.css", adds: 1, dels: 1, lines: stageLines } });
        send(
            putRow(
                message("a1", [
                    toolPart(
                        "tool-1",
                        {
                            kind: "edit",
                            title: "src/styles/stage.css",
                            status: "completed",
                            path: "/repo/src/styles/stage.css:2",
                            row: { action: "edit", target: "stage.css", path: "/repo/src/styles/stage.css:2" },
                        },
                        { diff: { path: "src/styles/stage.css", adds: 1, dels: 1 } },
                    ),
                ]),
            ),
        );

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
        expect(mocks.toolDetail).toHaveBeenCalledWith(agent.id, "tool-1");
    });

    it("opens an edit onto the hunk it wrote, and a failure onto why", async () => {
        await openTranscript();
        mocks.toolDetail.mockImplementation(async (_agentId, toolCallId) =>
            toolCallId === "tool-1"
                ? { diff: { path: "src/styles/stage.css", adds: 1, dels: 1, lines: stageLines } }
                : { output: { text: "1 failed · expected rgba(26,22,36,.72)", cut: false } },
        );
        send(
            putRow(
                message("a1", [
                    toolPart(
                        "tool-1",
                        {
                            kind: "edit",
                            title: "src/styles/stage.css",
                            status: "completed",
                            row: { action: "edit", target: "stage.css", path: "src/styles/stage.css" },
                        },
                        { diff: { path: "src/styles/stage.css", adds: 1, dels: 1 } },
                    ),
                    toolPart(
                        "tool-2",
                        { kind: "execute", title: "pnpm vitest run", status: "failed", row: { action: "execute", target: "pnpm vitest run" } },
                        { output: { lines: 1, cut: false, image: false } },
                    ),
                ]),
            ),
        );

        const edit = await findElement(".chat-tool[data-kind='edit']");
        expect(edit).toHaveTextContent("+1");
        expect(edit).toHaveTextContent("−1");
        expect(screen.queryByText(/background: transparent;/)).not.toBeInTheDocument();

        fireEvent.click(edit);
        const added = await findElement(".chat-diff-line.add");
        expect(added).toHaveTextContent("background: transparent;");
        // Only the part that changed is marked, not the whole line.
        expect(added.querySelector("mark")).toHaveTextContent("transparent");
        expect(document.querySelector(".chat-diff-line.del")).toHaveTextContent("background: var(--pane);");

        fireEvent.click(document.querySelector(".chat-tool.status-failed") as HTMLElement);
        expect(await screen.findByText(/1 failed/)).toBeInTheDocument();
    });

    it("says what a running command is for while it runs, and counts the run again once it ends", async () => {
        await openTranscript();
        const build = (status: string) =>
            toolPart("tool-1", {
                kind: "execute",
                title: "pnpm build",
                status,
                rawInput: { command: "pnpm build" },
                description: "Build the site",
                row: { action: "execute", target: "pnpm build" },
            });
        send(putRow(message("a1", [build("in_progress")])), head({ activeTool: "tool-1" }));

        const header = await screen.findByRole("button", { name: /Build the site/ });
        expect(header).toHaveClass("live");
        expect(document.querySelector(".chat-tool.live")).not.toBeNull();

        send(putPart("a1", build("completed")), head({ activeTool: null }));
        const finished = await screen.findByRole("button", { name: /1 tool call/ });
        expect(finished).not.toHaveClass("live");
        expect(document.querySelector(".chat-tool.live")).toBeNull();
    });

    it("opens a command onto the whole of it and what it printed, folding a long output", async () => {
        await openTranscript();
        const printed = Array.from({ length: 20 }, (_, index) => `line ${index + 1}`).join("\n");
        mocks.toolDetail.mockResolvedValue({ output: { text: printed, cut: false } });
        send(
            putRow(
                message("a1", [
                    toolPart(
                        "tool-1",
                        {
                            kind: "execute",
                            title: "python3 - <<'EOF'\nprint('hi')\nEOF",
                            status: "completed",
                            row: { action: "execute", target: "python3 - <<'EOF'" },
                        },
                        { output: { lines: 20, cut: false, image: false } },
                    ),
                ]),
            ),
        );

        const row = await findElement(".chat-tool");
        expect(row).toHaveTextContent("python3 - <<'EOF'");
        expect(row).not.toHaveTextContent("print('hi')");

        fireEvent.click(row);
        const terminal = await findElement(".chat-tool-terminal");
        expect(terminal.querySelector(".chat-tool-command pre")).toHaveTextContent("print('hi')");
        await waitFor(() => expect(terminal.querySelector(".chat-tool-output pre")).not.toBeNull());
        expect(screen.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Copy output" })).toBeInTheDocument();
        expect(terminal.querySelector(".chat-tool-output pre")).toHaveTextContent("line 12");
        expect(terminal.querySelector(".chat-tool-output pre")).not.toHaveTextContent("line 13");

        fireEvent.click(screen.getByRole("button", { name: "Show all 20 lines" }));
        expect(terminal.querySelector(".chat-tool-output pre")).toHaveTextContent("line 20");
    });

    it("opens a picture an agent sent, with a name to save it under", async () => {
        await openTranscript();
        send(putRow(message("a1", [{ kind: "content", id: "a1.0", content: { type: "image", mimeType: "image/png", data: "SEVMTE8=" } }])));

        const picture = await screen.findByRole("img", { name: "attachment.png" });
        expect(picture).toHaveAttribute("src", "data:image/png;base64,SEVMTE8=");

        fireEvent.click(picture);
        expect(shownImage()).toEqual({ src: "data:image/png;base64,SEVMTE8=", name: "attachment.png", path: undefined });
    });

    it("shows markup the person pasted, and still drops markup the agent wrote", async () => {
        await openTranscript('Use this: “<svg viewBox="0 0 16 16"><path d="M0 0h16"/></svg>”\n\n<div>own line</div>');
        send(putRow(message("a1", [text("a1.0", "Done <b>quietly</b>")])));

        const typed = await screen.findByText(/Use this:/);
        expect(typed.textContent).toBe('Use this: “<svg viewBox="0 0 16 16"><path d="M0 0h16"/></svg>”');
        expect(screen.getByText("<div>own line</div>")).toBeInTheDocument();
        expect(document.querySelector(".chat-message.user .chat-markdown svg")).toBeNull();
        expect(await screen.findByText(/Done/)).toHaveTextContent(/^Done quietly$/);
    });

    it("draws an answer as it streams: a fence still open, then the table after it", async () => {
        await openTranscript();
        const chunk = (said: string) => send(append("a1", "a1.0", said));

        send(putRow(message("a1", [text("a1.0", "")])));
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

        send(head({ running: false, stopReason: "end_turn" }));
        await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
        expect(screen.getByText("Here is the fix:")).toBe(intro);
        expect(document.querySelectorAll(".chat-markdown pre")).toHaveLength(1);
    });

    it("titles a fence whose name is not valid percent-encoding with the name as written", async () => {
        await openTranscript();
        send(putRow(message("a1", [text("a1.0", "```100%\nfull\n```\n")])));

        await waitFor(() => expect(document.querySelector(".chat-code-title")).toHaveTextContent("100%"));
    });

    it("keeps a subagent over the composer past a cancelled turn and settles its card when it stops", async () => {
        await openTranscript();
        const explorer: SubagentCard = {
            sessionId: "subagent-1",
            name: "Explore",
            task: "You are implementing performance fixes\nin the sikemux desktop app repo",
            state: "running",
            calls: 1,
            lastTool: { toolCallId: "tool-1", kind: "search", title: "usePty", status: "in_progress", row: searchPty },
        };
        send(
            putRow(message("a1", [subagentPart(explorer)])),
            putRow(message("s1", [toolPart("tool-1", { kind: "search", title: "usePty", status: "in_progress", row: searchPty })]), "subagent-1"),
            head({ runningSubagents: [explorer] }),
        );

        const strip = await screen.findByLabelText("1 subagent");
        expect(strip).toHaveTextContent("Explore");
        expect(strip).toHaveTextContent("search usePty");

        const card = document.querySelector(".chat-subagent") as HTMLElement;
        expect(card.querySelector('[role="img"][aria-label="working"]')).not.toBeNull();
        expect(card).toHaveTextContent("1 call");
        // The task is a whole prompt, so the row shows its first line only.
        expect(card).toHaveTextContent("You are implementing performance fixes");
        expect(card).not.toHaveTextContent("in the sikemux desktop app repo");

        send(head({ running: false, stopReason: "cancelled" }));
        expect(screen.getByLabelText("1 subagent")).toBeInTheDocument();

        const stopped: SubagentCard = { ...explorer, state: "cancelled" };
        send(
            putPart("a1", subagentPart(stopped)),
            putPart("s1", toolPart("tool-1", { kind: "search", title: "usePty", status: "cancelled", row: searchPty })),
            head({ runningSubagents: [] }),
        );

        await waitFor(() => expect(screen.queryByLabelText("1 subagent")).not.toBeInTheDocument());
        expect(document.querySelector('.chat-subagent [role="img"][aria-label="stopped"]')).not.toBeNull();
        expect(document.querySelector(".chat-tool-spinner")).toBeNull();
    });

    it("shows the agent starting and waits to be asked before starting a failed agent again", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await connected();

        send(head({ connection: "starting" }));
        expect(document.querySelector(".chat-welcome")).not.toBeNull();
        send(head({ connection: "error", error: "agent failed", ended: { reason: "failed", message: "agent failed" } }));
        expect(screen.getByText("Structured session unavailable.")).toBeInTheDocument();
        expect(mocks.start).toHaveBeenCalledTimes(1);

        fireEvent.click(screen.getByRole("button", { name: "Reconnect" }));
        await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(2));
    });

    describe("an agent that dies under the chat", () => {
        const saved = { ...agent, resumeId: "session-1" };

        async function openReady(chatAgent: Agent) {
            render(<AgentChatPane agent={chatAgent} cwd="/repo" active visible profile={undefined} onBusyChange={() => {}} />);
            await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(1));
            await connected();
        }

        it("shows one the core gave up starting again, with the detail and a retry", async () => {
            let resumed!: () => void;
            mocks.start.mockImplementationOnce(async () => ({}));
            mocks.start.mockImplementationOnce(
                () =>
                    new Promise((resolve) => {
                        resumed = () => resolve({});
                    }),
            );
            await openReady(saved);
            send(head({ connection: "error", running: false, ended: { reason: "exited", message: "Process exited with 1: out of memory" } }));
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
            send(head({ connection: "stopped", running: false, ended: { reason: "requested" } }));
            await act(async () => {});
            expect(mocks.start).toHaveBeenCalledTimes(1);
            expect(screen.queryByText("Couldn't resume this chat")).not.toBeInTheDocument();
        });

        it("says so when a retry fails to start", async () => {
            mocks.start.mockImplementationOnce(async () => ({}));
            mocks.start.mockImplementationOnce(async () => {
                throw new Error("session not found");
            });
            await openReady(saved);
            send(head({ connection: "stopped", running: false, ended: { reason: "exited" } }));
            fireEvent.click(screen.getByRole("button", { name: "Retry" }));

            expect(await screen.findByText("session not found")).toBeInTheDocument();
            expect(screen.getByText("Couldn't resume this chat")).toBeInTheDocument();
            expect(mocks.start).toHaveBeenCalledTimes(2);
        });

        it("cannot resume a chat with no saved session, and retries with a fresh one", async () => {
            await openReady(agent);
            send(head({ connection: "stopped", running: false, ended: { reason: "exited" } }));
            expect(screen.getByText("Couldn't resume this chat")).toBeInTheDocument();
            await act(async () => {});
            expect(mocks.start).toHaveBeenCalledTimes(1);

            fireEvent.click(screen.getByRole("button", { name: "Retry" }));
            await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(2));
        });

        it("sends a message written while it resumes once it is back", async () => {
            await openReady(saved);
            send(head({ connection: "stopped", running: false, ended: { reason: "exited" } }));
            fireEvent.click(screen.getByRole("button", { name: "Retry" }));

            const editor = screen.getByRole("textbox", { name: "Message agent" });
            typeDraft(editor, "carry on");
            fireEvent.keyDown(editor, { key: "Enter" });

            await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "carry on", [], [], expect.any(String)));
        });
    });

    it("shows permission requests even when the tool has no title", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await connected();
        send(
            head({
                permissions: [
                    {
                        requestId: "request-1",
                        sessionId: "session-1",
                        toolCall: { toolCallId: "tool-1", title: "" },
                        options: [{ optionId: "allow", name: "Allow once", kind: "allow_once" }],
                    },
                ],
            }),
        );
        fireEvent.click(await screen.findByRole("button", { name: "Allow once" }));
        await waitFor(() => expect(acpApi.permissionReply).toHaveBeenCalledWith(agent.id, "request-1", "allow"));
    });

    it("lists a background task until it ends, and stops it on request", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await connected();
        send(
            head({
                tasks: [
                    { asyncTaskId: "task-1", name: "pnpm test", taskType: "shell", description: "Run the suite", state: "running", canStop: true },
                ],
            }),
        );
        expect(await screen.findByRole("button", { name: "Stop pnpm test" })).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Stop pnpm test" }));
        await waitFor(() => expect(mocks.stopTask).toHaveBeenCalledWith("agent-1", "task-1"));

        send(head({ tasks: [] }));
        await waitFor(() => expect(screen.queryByRole("button", { name: "Stop pnpm test" })).not.toBeInTheDocument());
    });

    it("says a background task's name once when its description repeats it", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active onBusyChange={() => {}} />);
        await connected();
        send(
            head({
                tasks: [
                    {
                        asyncTaskId: "task-1",
                        name: "push gate failures",
                        description: "push gate failures",
                        taskType: "shell",
                        state: "running",
                        canStop: false,
                    },
                ],
            }),
        );

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

    it("holds what changes before the chat is taken up, and shows it once it is", async () => {
        let answer: (head: Partial<ChatHead>) => void = () => {};
        mocks.start.mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
        render(<AgentChatPane agent={{ ...agent, resumeId: "old-session" }} cwd="/repo" active visible onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.start).toHaveBeenCalled());

        send(putRow(message("u0", [text("u0.0", "Earlier question")], "user")));
        await nextFrame();
        expect(screen.queryByText("Earlier question")).not.toBeInTheDocument();

        await act(async () => answer({}));
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
        await connected();
        await nextFrame();
        expect(elsewhere).toHaveFocus();
        elsewhere.remove();
    });

    it("routes native path drops into prompt attachments", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await connected();

        const editor = screen.getByRole("textbox", { name: "Message agent" });
        expect(dispatchPathDrop(editor, ["/repo/src/App.tsx"])).toBe(true);
        expect(await screen.findByRole("button", { name: "Remove App.tsx" })).toBeInTheDocument();

        typeDraft(editor, "Review this");
        fireEvent.click(screen.getByRole("button", { name: "Send message" }));
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "Review this", ["/repo/src/App.tsx"], [], expect.any(String)));
    });
});
