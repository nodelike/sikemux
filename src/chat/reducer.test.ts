import { describe, expect, it } from "vitest";
import { chatReducer, initialChatState } from "./reducer";
import type { ChatState } from "./types";

const ROOT_SESSION = "session-1";

function update(state: ChatState, value: Record<string, unknown>, sessionId = ROOT_SESSION): ChatState {
    return chatReducer(state, { type: "session_update", sessionId, update: value });
}

describe("chat reducer", () => {
    it("keeps the context window the agent reports and ignores a malformed one", () => {
        const claude = update(initialChatState, {
            sessionUpdate: "usage_update",
            used: 84_000,
            size: 200_000,
            cost: { amount: 1.25, currency: "USD" },
        });
        expect(claude.usage).toEqual({ used: 84_000, size: 200_000, cost: { amount: 1.25, currency: "USD" } });

        const codex = update(claude, { sessionUpdate: "usage_update", used: 90_000, size: 272_000 });
        expect(codex.usage).toEqual({ used: 90_000, size: 272_000 });

        expect(update(codex, { sessionUpdate: "usage_update", used: 1, size: 0 })).toBe(codex);
        expect(update(codex, { sessionUpdate: "usage_update", used: "lots", size: 10 })).toBe(codex);
    });

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
        expect(part.tool).toMatchObject({ toolCallId: "tool-1", title: "Read file", status: "completed" });
        expect(part.tool).not.toHaveProperty("rawOutput");
    });

    it("reads a finished call once and keeps only what the transcript shows", () => {
        const created = update(initialChatState, {
            sessionUpdate: "tool_call",
            toolCallId: "tool-1",
            title: "Edit file",
            kind: "edit",
            status: "in_progress",
            rawInput: { file_path: "/repo/notes.md", old_string: "one\n", new_string: "two\n" },
        });
        const completed = update(created, { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "completed" });
        const part = completed.messages[0].parts[0];

        expect(part.kind).toBe("tool");
        if (part.kind !== "tool") throw new Error("expected tool part");
        expect(part.diff).toMatchObject({ path: "/repo/notes.md", adds: 1, dels: 1 });
        expect(part.tool).not.toHaveProperty("rawInput");
    });

    it("keeps a failed call's message and drops the output it came from", () => {
        const created = update(initialChatState, {
            sessionUpdate: "tool_call",
            toolCallId: "tool-2",
            title: "Run tests",
            kind: "execute",
            status: "in_progress",
        });
        const failed = update(created, {
            sessionUpdate: "tool_call_update",
            toolCallId: "tool-2",
            status: "failed",
            rawOutput: { stderr: "  2 tests failed  " },
        });
        const part = failed.messages[0].parts[0];

        expect(part.kind).toBe("tool");
        if (part.kind !== "tool") throw new Error("expected tool part");
        expect(part.failure).toBe("2 tests failed");
        expect(part.tool).not.toHaveProperty("rawOutput");
    });

    it("keeps a picture too big to hold by name rather than by its bytes", () => {
        const streamed = update(initialChatState, {
            sessionUpdate: "agent_message_chunk",
            messageId: "remote-1",
            content: { type: "image", mimeType: "image/png", data: "A".repeat(3 * 1024 * 1024), uri: "file:///tmp/shot.png" },
        });
        const part = streamed.messages[0].parts[0];

        expect(part.kind).toBe("content");
        if (part.kind !== "content") throw new Error("expected content part");
        expect(part.content).not.toHaveProperty("data");
        expect(part.content.uri).toBe("file:///tmp/shot.png");
    });

    it("streams a subagent session into its own thread", () => {
        const spawned = update(initialChatState, {
            sessionUpdate: "subagent_spawned",
            subagentSessionId: "subagent-1",
            name: "Explore",
            task: "Find the ACP adapter",
        });
        const streamed = update(
            spawned,
            { sessionUpdate: "agent_message_chunk", messageId: "child-1", content: { type: "text", text: "Looking" } },
            "subagent-1",
        );
        const finished = update(streamed, {
            sessionUpdate: "subagent_state_update",
            subagentSessionId: "subagent-1",
            state: "completed",
        });

        const part = finished.messages[0].parts[0];
        expect(part.kind).toBe("subagent");
        if (part.kind !== "subagent") throw new Error("expected subagent part");
        expect(part.subagent.state).toBe("completed");
        expect(part.subagent.messages[0].parts).toEqual([{ id: "child-1-text-0", kind: "text", text: "Looking" }]);
    });

    it("keeps subagent output out of the parent transcript", () => {
        const spawned = update(initialChatState, { sessionUpdate: "subagent_spawned", subagentSessionId: "subagent-1", name: "Explore", task: "" });
        const streamed = update(spawned, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "nested" } }, "subagent-1");

        expect(streamed.messages).toHaveLength(1);
        expect(streamed.messages[0].parts).toHaveLength(1);
    });

    it("ignores a tool update for a call it never saw open", () => {
        const orphan = update(initialChatState, { sessionUpdate: "tool_call_update", toolCallId: "tool-9", status: "in_progress" });

        expect(orphan.messages).toEqual([]);
    });

    it("stops the work a finished turn left mid-flight", () => {
        const spawned = update(initialChatState, {
            sessionUpdate: "subagent_spawned",
            subagentSessionId: "subagent-1",
            name: "Explore",
            task: "Look around",
        });
        const nested = update(spawned, { sessionUpdate: "tool_call", toolCallId: "tool-2", title: "Grep", status: "in_progress" }, "subagent-1");
        const parent = update(nested, { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Task", status: "pending" });
        const ended = chatReducer(parent, { type: "turn_completed", stopReason: "cancelled" });

        const [subagentPart, toolPart] = ended.messages[0].parts;
        if (subagentPart.kind !== "subagent" || toolPart.kind !== "tool") throw new Error("expected a subagent beside a tool call");
        expect(subagentPart.subagent.state).toBe("cancelled");
        expect(toolPart.tool.status).toBe("cancelled");
        expect(toolPart.endedAt).toBeDefined();

        const nestedPart = subagentPart.subagent.messages[0].parts[0];
        if (nestedPart.kind !== "tool") throw new Error("expected the subagent's own call");
        expect(nestedPart.tool.status).toBe("cancelled");
    });

    it("leaves a call that already answered alone when the turn ends", () => {
        const created = update(initialChatState, { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Read file", status: "completed" });
        const ended = chatReducer(created, { type: "turn_completed" });

        expect(ended.messages).toBe(created.messages);
    });

    it("tracks a background task until it reaches an end state", () => {
        const spawned = update(initialChatState, {
            sessionUpdate: "async_task_spawned",
            asyncTaskId: "task-1",
            name: "pnpm test",
            taskType: "shell",
            description: "Run the suite",
            canStop: true,
        });
        const progressed = update(spawned, { sessionUpdate: "async_task_progress", asyncTaskId: "task-1", summary: "12 files passed" });
        const finished = update(progressed, { sessionUpdate: "async_task_state_update", asyncTaskId: "task-1", state: "completed" });

        expect(spawned.tasks).toEqual([
            {
                asyncTaskId: "task-1",
                name: "pnpm test",
                taskType: "shell",
                description: "Run the suite",
                state: "running",
                canStop: true,
                outputFilePath: undefined,
            },
        ]);
        expect(progressed.tasks[0].summary).toBe("12 files passed");
        expect(finished.tasks).toEqual([]);
        expect(finished.messages.at(-1)?.parts.at(-1)).toEqual({
            id: "notice-task-1",
            kind: "notice",
            notice: { name: "pnpm test", state: "completed", summary: "12 files passed" },
        });
    });

    it("keeps a harness notification out of the transcript", () => {
        const notified = update(initialChatState, {
            sessionUpdate: "user_message_chunk",
            content: {
                type: "text",
                text: "<task-notification>\n<task-id>b9u0</task-id>\n<event>audit</event>\n</task-notification>",
            },
        });

        expect(notified.messages).toEqual([]);
    });

    it("keeps Claude's interrupt marker out of the transcript", () => {
        const replayed = ["[Request interrupted by user]", "[Request interrupted by user for tool use]"].reduce(
            (state, text) => update(state, { sessionUpdate: "user_message_chunk", content: { type: "text", text } }),
            initialChatState,
        );

        expect(replayed.messages).toEqual([]);
    });

    it("keeps a message that only mentions a tag", () => {
        const asked = update(initialChatState, {
            sessionUpdate: "user_message_chunk",
            content: { type: "text", text: "why does <b>bold</b> render oddly" },
        });

        expect(asked.messages.at(-1)?.parts).toEqual([{ id: "user-fallback-1-text-0", kind: "text", text: "why does <b>bold</b> render oddly" }]);
    });

    it("drops background tasks when the session stops", () => {
        const spawned = update(initialChatState, { sessionUpdate: "async_task_spawned", asyncTaskId: "task-1", name: "pnpm test", canStop: true });
        const stopped = chatReducer(spawned, { type: "status", state: "stopped" });

        expect(stopped.tasks).toEqual([]);
    });

    it("runs a turn the agent starts on its own until its closing usage report", () => {
        const ready = chatReducer(initialChatState, { type: "ready", capabilities: {}, setup: {} });
        const woken = update(ready, {
            sessionUpdate: "tool_call",
            toolCallId: "call-1",
            title: "git log",
            status: "in_progress",
        });
        expect(woken).toMatchObject({ running: true, unprompted: true });

        const report = { sessionUpdate: "usage_update", used: 1_000, size: 200_000 };
        expect(update(woken, report).running).toBe(true);

        const ended = update(woken, { ...report, _meta: { "_claude/origin": { kind: "peer" } } });
        expect(ended).toMatchObject({ running: false, unprompted: false, usage: { used: 1_000, size: 200_000 } });
        const part = ended.messages[0].parts[0];
        expect(part.kind === "tool" && part.tool.status).toBe("cancelled");
    });

    it("does not mistake a resumed session's replay for a turn", () => {
        const replayed = update(initialChatState, {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "Earlier answer" },
        });
        expect(replayed.running).toBe(false);
    });

    it("leaves a prompted turn to its own completion", () => {
        const ready = chatReducer(initialChatState, { type: "ready", capabilities: {}, setup: {} });
        const prompted = chatReducer(ready, { type: "turn_started" });
        const reported = update(prompted, {
            sessionUpdate: "usage_update",
            used: 1,
            size: 10,
            _meta: { "_claude/origin": { kind: "task-notification" } },
        });
        expect(reported.running).toBe(true);
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

    it("holds the transcript through a reconnect until the resumed session replays it", () => {
        const before = update(initialChatState, {
            sessionUpdate: "agent_message_chunk",
            messageId: "message-1",
            content: { type: "text", text: "Earlier answer" },
        });
        const reconnecting = chatReducer(before, { type: "reset", hold: true });
        expect(reconnecting.messages).toEqual(before.messages);

        const replayed = update(reconnecting, {
            sessionUpdate: "agent_message_chunk",
            messageId: "message-1",
            content: { type: "text", text: "Earlier answer" },
        });

        expect(replayed.messages).toHaveLength(1);
        expect(replayed.messages[0].parts).toEqual([{ id: "message-1-text-0", kind: "text", text: "Earlier answer" }]);
        expect(chatReducer(before, { type: "reset" }).messages).toEqual([]);
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
