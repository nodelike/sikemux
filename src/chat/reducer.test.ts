import { describe, expect, it } from "vitest";
import { chatReducer, initialChatState } from "./reducer";
import type { ChatState } from "./types";

function update(state: ChatState, value: Record<string, unknown>): ChatState {
    return chatReducer(state, { type: "session_update", update: value });
}

describe("chat reducer", () => {
    it("merges streamed text chunks by ACP message id", () => {
        const first = update(initialChatState, {
            sessionUpdate: "agent_message_chunk",
            messageId: "message-1",
            content: { type: "text", text: "Hello" },
        });
        const second = update(first, {
            sessionUpdate: "agent_message_chunk",
            messageId: "message-1",
            content: { type: "text", text: " world" },
        });

        expect(second.messages).toHaveLength(1);
        expect(second.messages[0].parts).toEqual([{ id: "message-1-text-0", kind: "text", text: "Hello world" }]);
    });

    it("keeps consecutive chunks without message ids in one message", () => {
        const first = update(initialChatState, {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "one" },
        });
        const second = update(first, {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: " two" },
        });

        expect(second.messages).toHaveLength(1);
        expect(second.messages[0].parts).toEqual([{ id: "assistant-fallback-1-text-0", kind: "text", text: "one two" }]);
    });

    it("suppresses agent replay of an optimistic user prompt", () => {
        const local = chatReducer(initialChatState, {
            type: "local_prompt",
            text: "Check this",
            paths: ["/tmp/example.ts"],
        });
        const echoed = update(local, {
            sessionUpdate: "user_message_chunk",
            messageId: "remote-user-1",
            content: { type: "text", text: "Check this" },
        });
        const answered = update(echoed, {
            sessionUpdate: "agent_message_chunk",
            messageId: "remote-agent-1",
            content: { type: "text", text: "Done" },
        });

        expect(answered.messages.map((message) => message.role)).toEqual(["user", "assistant"]);
        expect(answered.messages[0].attachments).toEqual(["/tmp/example.ts"]);
    });

    it("merges tool updates into their original call", () => {
        const created = update(initialChatState, {
            sessionUpdate: "tool_call",
            toolCallId: "tool-1",
            title: "Read file",
            kind: "read",
            status: "pending",
        });
        const completed = update(created, {
            sessionUpdate: "tool_call_update",
            toolCallId: "tool-1",
            status: "completed",
            rawOutput: { bytes: 42 },
        });
        const part = completed.messages[0].parts[0];

        expect(part.kind).toBe("tool");
        if (part.kind !== "tool") throw new Error("expected tool part");
        expect(part.tool).toMatchObject({
            toolCallId: "tool-1",
            title: "Read file",
            status: "completed",
            rawOutput: { bytes: 42 },
        });
    });

    it("replaces slash commands when ACP sends a new command list", () => {
        const state = update(initialChatState, {
            sessionUpdate: "available_commands_update",
            availableCommands: [
                { name: "compact", description: "Compact context", input: { hint: "focus" } },
                { name: "help", description: "Show help" },
                { broken: true },
            ],
        });

        expect(state.commands).toEqual([
            { name: "compact", description: "Compact context", input: { hint: "focus" } },
            { name: "help", description: "Show help" },
        ]);
    });

    it("keeps independent permission requests until each reply completes", () => {
        const request = {
            requestId: "permission-1",
            sessionId: "session-1",
            toolCall: { toolCallId: "tool-1", title: "Run tests" },
            options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
        };
        const pending = chatReducer(initialChatState, { type: "permission_requested", request });
        const cleared = chatReducer(pending, { type: "permission_cleared", requestId: request.requestId });

        expect(pending.permissions).toEqual([request]);
        expect(cleared.permissions).toEqual([]);
    });
});
