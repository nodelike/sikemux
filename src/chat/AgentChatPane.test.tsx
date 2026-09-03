import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AcpEvent } from "../api/acp";
import { dispatchPathDrop } from "../state/dropRegistry";
import type { Agent } from "../state/types";
import { AgentChatPane } from "./AgentChatPane";

const mocks = vi.hoisted(() => ({
    eventListener: null as ((event: AcpEvent) => void) | null,
    prompt: vi.fn(async () => {}),
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
        stop: vi.fn(async () => {}),
        prompt: mocks.prompt,
        cancel: vi.fn(async () => {}),
        permissionReply: vi.fn(async () => {}),
    },
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
    mocks.prompt.mockClear();
    mocks.start.mockClear();
    mocks.eventListener = null;
});

afterEach(cleanup);

describe("AgentChatPane", () => {
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
