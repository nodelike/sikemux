import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { acpApi, type AcpEvent } from "../api/acp";
import { dispatchPathDrop } from "../state/dropRegistry";
import type { Agent } from "../state/types";
import { AgentChatPane } from "./AgentChatPane";

const mocks = vi.hoisted(() => ({
    eventListener: null as ((event: AcpEvent) => void) | null,
    prompt: vi.fn(async () => {}),
    setPermissionMode: vi.fn(async () => {}),
    setConfig: vi.fn(),
    setAgentModelPreferences: vi.fn(),
    attachAgentSession: vi.fn(),
    setAgentPermissionMode: vi.fn(),
    noteAcpAgentState: vi.fn(),
    start: vi.fn(async () => ({ sessionId: "session-1", capabilities: {}, setup: {} })),
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
        cancel: vi.fn(async () => {}),
        permissionReply: vi.fn(async () => {}),
    },
}));

vi.mock("../state/commands", () => ({
    attachAgentSession: mocks.attachAgentSession,
    setAgentPermissionMode: mocks.setAgentPermissionMode,
    setAgentModelPreferences: mocks.setAgentModelPreferences,
    setAgentTitle: vi.fn(),
    noteAcpAgentState: mocks.noteAcpAgentState,
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

function emit(kind: AcpEvent["kind"], payload: Record<string, unknown>): void {
    act(() => mocks.eventListener?.({ agentId: agent.id, kind, payload }));
}

beforeEach(() => {
    vi.clearAllMocks();
    mocks.eventListener = null;
});

afterEach(cleanup);

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
        await waitFor(() => expect(screen.getByRole("button", { name: "Agent" })).toBeEnabled());
        emit("session_update", { update: { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Existing message" } } });
        emit("ready", { capabilities: {}, setup: {} });
        await waitFor(() => expect(screen.getByRole("button", { name: "Agent" })).toBeDisabled());
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

    it("blocks a second prompt while the first is waiting for turn_started", async () => {
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

    it("shows adapter progress and retries failed startup", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());

        emit("status", { state: "installing" });
        expect(screen.getAllByText("Installing structured-session adapter…")[0]).toBeInTheDocument();
        emit("error", { message: "adapter failed" });
        const callsBeforeRetry = mocks.start.mock.calls.length;
        fireEvent.click(screen.getByRole("button", { name: "Retry" }));

        await waitFor(() => expect(mocks.start.mock.calls.length).toBeGreaterThan(callsBeforeRetry));
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

    it("shows ACP slash commands and inserts the selected command", async () => {
        render(<AgentChatPane agent={agent} cwd="/repo" active profile={undefined} onBusyChange={() => {}} />);
        await waitFor(() => expect(mocks.eventListener).not.toBeNull());
        emit("ready", { capabilities: {}, setup: {} });
        emit("session_update", {
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
