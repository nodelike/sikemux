import { afterEach, describe, expect, it, vi } from "vitest";
import { chatReducer, initialChatState } from "./reducer";
import type { ChatState } from "./types";

const ROOT_SESSION = "session-1";

function update(state: ChatState, value: Record<string, unknown>, sessionId = ROOT_SESSION): ChatState {
    return chatReducer(state, { type: "session_update", sessionId, update: value });
}

function replay(state: ChatState, rows: Record<string, unknown>[], sessionId = ROOT_SESSION): ChatState {
    return rows.reduce((current, row) => update(current, row, sessionId), state);
}

function toolPart(state: ChatState, messageIndex = 0, partIndex = 0) {
    const part = state.messages[messageIndex].parts[partIndex];
    if (part.kind !== "tool") throw new Error(`expected a tool part, got ${part.kind}`);
    return part;
}

afterEach(() => {
    vi.useRealTimers();
});

/* Times say when this side saw something happen, so they differ between a
   client that watched and one that replayed. */
function withoutTimes(value: unknown): unknown {
    if (Array.isArray(value)) return value.map(withoutTimes);
    if (typeof value !== "object" || value === null) return value;
    return Object.fromEntries(
        Object.entries(value)
            .filter(([key]) => !/At$|^stream/.test(key))
            .map(([key, entry]) => [key, withoutTimes(entry)]),
    );
}

describe("chat reducer", () => {
    it("rebuilds from a core's replay, with streamed text joined, the transcript a watching client built", () => {
        const say = (kind: string, text: string) => ({ sessionUpdate: kind, content: { type: "text", text } });
        const tool = { sessionUpdate: "tool_call", toolCallId: "call-1", title: "Read file", status: "in_progress" };
        const toolDone = { sessionUpdate: "tool_call_update", toolCallId: "call-1", status: "completed" };
        const watch = (state: ChatState, steps: Array<Record<string, unknown> | "turn_started" | "turn_completed">) =>
            steps.reduce(
                (current, step) =>
                    step === "turn_started"
                        ? chatReducer(current, { type: "turn_started" })
                        : step === "turn_completed"
                          ? chatReducer(current, { type: "turn_completed", stopReason: "end_turn" })
                          : update(current, step),
                state,
            );
        const watched = watch(initialChatState, [
            say("user_message_chunk", "Look"),
            "turn_started",
            say("agent_thought_chunk", "Hm"),
            say("agent_thought_chunk", "m."),
            say("agent_message_chunk", "Rea"),
            say("agent_message_chunk", "ding."),
            tool,
            toolDone,
            say("agent_message_chunk", "Do"),
            say("agent_message_chunk", "ne."),
            "turn_completed",
        ]);
        const replayed = watch(chatReducer({ ...initialChatState, messages: watched.messages }, { type: "reset", hold: true }), [
            say("user_message_chunk", "Look"),
            "turn_started",
            say("agent_thought_chunk", "Hmm."),
            say("agent_message_chunk", "Reading."),
            tool,
            toolDone,
            say("agent_message_chunk", "Done."),
            "turn_completed",
        ]);
        expect(withoutTimes(replayed.messages)).toEqual(withoutTimes(watched.messages));
        expect(replayed.running).toBe(false);
        expect(replayed.awaitingReplay).toBe(false);
    });

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
            title: "Write notes.md",
            kind: "edit",
            status: "in_progress",
        });
        const failed = update(created, {
            sessionUpdate: "tool_call_update",
            toolCallId: "tool-2",
            status: "failed",
            rawOutput: { stderr: "  permission denied  " },
        });
        const part = failed.messages[0].parts[0];

        expect(part.kind).toBe("tool");
        if (part.kind !== "tool") throw new Error("expected tool part");
        expect(part.failure).toBe("permission denied");
        expect(part.tool).not.toHaveProperty("rawOutput");
    });

    it("keeps what a finished command printed, and Codex's exit code with it", () => {
        const created = update(initialChatState, {
            sessionUpdate: "tool_call",
            toolCallId: "tool-3",
            title: "pnpm vitest run",
            kind: "execute",
            status: "in_progress",
            rawInput: { command: "pnpm vitest run", description: "Run the tests" },
        });
        const failed = update(created, {
            sessionUpdate: "tool_call_update",
            toolCallId: "tool-3",
            status: "failed",
            rawOutput: { formatted_output: " Tests  2 failed | 40 passed\n", exit_code: 1 },
        });
        const part = failed.messages[0].parts[0];

        expect(part.kind).toBe("tool");
        if (part.kind !== "tool") throw new Error("expected tool part");
        expect(part.output).toEqual({ text: " Tests  2 failed | 40 passed", cut: false, exitCode: 1 });
        expect(part.failure).toBeUndefined();
        expect(part.tool).not.toHaveProperty("rawOutput");
        expect(part.tool).not.toHaveProperty("rawInput");
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

    it("keeps a background agent's notice out even when its report has paragraphs", () => {
        const notified = update(initialChatState, {
            sessionUpdate: "user_message_chunk",
            content: {
                type: "text",
                text: [
                    "<task-notification>\n<task-id>a41</task-id>\n<status>completed</status>",
                    "<result>Research is done.\n\n## Blocking bugs\n\n- one\n- two</result>\n</task-notification>",
                    "<system-reminder>\nNot user input.\n\nIgnore.\n</system-reminder>",
                ].join("\n"),
            },
        });

        expect(notified.messages).toEqual([]);
    });

    it("keeps pasted markup with paragraphs that the harness did not write", () => {
        const pasted = update(initialChatState, {
            sessionUpdate: "user_message_chunk",
            content: { type: "text", text: "<div>\n\nwhy is this blank\n</div>" },
        });

        expect(pasted.messages).toHaveLength(1);
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

    it("does not mistake history replayed after the session is ready for a turn", () => {
        const ready = chatReducer(chatReducer(initialChatState, { type: "reset", hold: true }), { type: "ready", capabilities: {}, setup: {} });
        const replayed = [
            { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Run the pre-push checks" } },
            { sessionUpdate: "tool_call", toolCallId: "call-1", title: "pnpm check", status: "completed" },
            { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "All pushed." } },
        ].reduce((state, row) => update(state, row), ready);

        expect(replayed.running).toBe(false);
    });

    it("leaves ending a turn to the turn events, not the usage report", () => {
        const prompted = chatReducer(chatReducer(initialChatState, { type: "ready", capabilities: {}, setup: {} }), { type: "turn_started" });
        const reported = update(prompted, {
            sessionUpdate: "usage_update",
            used: 1,
            size: 10,
            _meta: { "_claude/origin": { kind: "task-notification" } },
        });
        expect(reported).toMatchObject({ running: true, usage: { used: 1, size: 10 } });
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

    it("splits a message into text, thought and content parts as the kind changes", () => {
        const streamed = replay(initialChatState, [
            { sessionUpdate: "agent_thought_chunk", messageId: "m1", content: { type: "text", text: "Thinking" } },
            { sessionUpdate: "agent_thought_chunk", messageId: "m1", content: { type: "text", text: " harder" } },
            { sessionUpdate: "agent_message_chunk", messageId: "m1", content: { type: "text", text: "Answer" } },
            { sessionUpdate: "agent_message_chunk", messageId: "m1", content: { type: "image", mimeType: "image/png", data: "AAAA" } },
            { sessionUpdate: "agent_message_chunk", messageId: "m1", content: { type: "text", text: "After" } },
        ]);

        expect(streamed.messages).toHaveLength(1);
        expect(streamed.messages[0].parts).toEqual([
            { id: "m1-thought-0", kind: "thought", text: "Thinking harder" },
            { id: "m1-text-1", kind: "text", text: "Answer" },
            { id: "m1-content-2", kind: "content", content: { type: "image", mimeType: "image/png", data: "AAAA" } },
            { id: "m1-text-3", kind: "text", text: "After" },
        ]);
    });

    it("opens a message with a picture and keeps a small one's bytes", () => {
        const streamed = update(initialChatState, {
            sessionUpdate: "agent_message_chunk",
            messageId: "m1",
            content: { type: "image", mimeType: "image/png", data: "AAAA" },
        });

        expect(streamed.messages[0].parts).toEqual([
            { id: "m1-content-0", kind: "content", content: { type: "image", mimeType: "image/png", data: "AAAA" } },
        ]);
    });

    it("drops a chunk that carries no content block", () => {
        const empty = update(initialChatState, { sessionUpdate: "agent_message_chunk", messageId: "m1" });
        const untyped = update(initialChatState, { sessionUpdate: "agent_message_chunk", content: { text: "no type" } });

        expect(empty).toBe(initialChatState);
        expect(untyped).toBe(initialChatState);
    });

    it("starts a new assistant message after the user's own prompt rather than extending it", () => {
        const prompted = chatReducer(initialChatState, { type: "local_prompt", text: "Hi", paths: [] });
        const answered = replay(prompted, [
            { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Hello" } },
            { sessionUpdate: "agent_message_chunk", content: { type: "text", text: " there" } },
        ]);

        expect(answered.messages.map((message) => message.role)).toEqual(["user", "assistant"]);
        expect(answered.messages[1].parts).toEqual([{ id: "assistant-fallback-2-text-0", kind: "text", text: "Hello there" }]);
    });

    it("times only the answer to a turn it watched run", () => {
        vi.useFakeTimers();
        vi.setSystemTime(1_000);
        const running = chatReducer(initialChatState, { type: "turn_started" });
        const first = update(running, { sessionUpdate: "agent_message_chunk", messageId: "m1", content: { type: "text", text: "Hel" } });
        vi.setSystemTime(1_500);
        const second = update(first, { sessionUpdate: "agent_message_chunk", messageId: "m1", content: { type: "text", text: "lo" } });

        expect(second.messages[0]).toMatchObject({ streamStartedAt: 1_000, streamEndedAt: 1_500, streamChars: 5 });

        const replayed = update(initialChatState, {
            sessionUpdate: "agent_message_chunk",
            messageId: "m1",
            content: { type: "text", text: "Hello" },
        });
        expect(replayed.messages[0].streamStartedAt).toBeUndefined();
    });

    it("drops a user chunk that is blank or a lone harness tag", () => {
        const dropped = replay(initialChatState, [
            { sessionUpdate: "user_message_chunk", content: { type: "text", text: "   \n" } },
            { sessionUpdate: "user_message_chunk", content: { type: "text", text: "<ide_opened_file>src/a.ts</ide_opened_file>" } },
            {
                sessionUpdate: "user_message_chunk",
                content: { type: "text", text: "<command-name>/clear</command-name>\n<command-args></command-args>" },
            },
        ]);

        expect(dropped.messages).toEqual([]);
    });

    it("keeps a user message whose harness tag is never closed", () => {
        const kept = update(initialChatState, {
            sessionUpdate: "user_message_chunk",
            content: { type: "text", text: "<system-reminder> left open" },
        });

        expect(kept.messages).toHaveLength(1);
    });

    it("replaces a call the agent announces again instead of merging into it", () => {
        const opened = update(initialChatState, {
            sessionUpdate: "tool_call",
            toolCallId: "tool-1",
            title: "Read a.ts",
            kind: "read",
            status: "pending",
            locations: [{ path: "/a.ts" }],
        });
        const reopened = update(opened, { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Read b.ts", status: "in_progress" });

        expect(reopened.messages[0].parts).toHaveLength(1);
        expect(toolPart(reopened).tool).toEqual({ sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Read b.ts", status: "in_progress" });
    });

    it("keeps a call's title when an update sends an empty one and stays open while it runs", () => {
        const opened = update(initialChatState, { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Run tests", status: "pending" });
        const progressed = update(opened, { sessionUpdate: "tool_call_update", toolCallId: "tool-1", title: "", status: "in_progress" });

        expect(toolPart(progressed).tool).toMatchObject({ title: "Run tests", status: "in_progress" });
        expect(toolPart(progressed).endedAt).toBeUndefined();
        expect(toolPart(progressed).startedAt).toBeDefined();
    });

    it("keeps a finished call's end time and drops payloads a late update sends", () => {
        vi.useFakeTimers();
        vi.setSystemTime(2_000);
        const finished = replay(initialChatState, [
            { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Grep", status: "in_progress" },
            { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "completed" },
        ]);
        vi.setSystemTime(9_000);
        const late = update(finished, { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "failed", rawOutput: { stderr: "late" } });

        expect(toolPart(late).endedAt).toBe(2_000);
        expect(toolPart(late).tool.status).toBe("failed");
        expect(toolPart(late).tool).not.toHaveProperty("rawOutput");
        expect(toolPart(late).failure).toBeUndefined();
    });

    it("settles a call first seen already finished without timing it", () => {
        const replayed = update(initialChatState, {
            sessionUpdate: "tool_call",
            toolCallId: "tool-1",
            title: "Grep",
            status: "failed",
            rawOutput: { stderr: "boom" },
        });

        expect(toolPart(replayed).startedAt).toBeUndefined();
        expect(toolPart(replayed).failure).toBe("boom");
    });

    it("ignores a call with no title or no id", () => {
        const noTitle = update(initialChatState, { sessionUpdate: "tool_call", toolCallId: "tool-1" });
        const noId = update(initialChatState, { sessionUpdate: "tool_call_update", status: "completed" });

        expect(noTitle).toBe(initialChatState);
        expect(noId).toBe(initialChatState);
    });

    it("cancels a call that never reported a status when the turn ends", () => {
        const opened = update(initialChatState, { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Grep" });
        const ended = chatReducer(opened, { type: "turn_completed" });

        expect(toolPart(ended).tool.status).toBe("cancelled");
        expect(ended.stopReason).toBeNull();
    });

    it("leaves a finished subagent and plain text alone when the turn ends", () => {
        const done = replay(initialChatState, [
            { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Sending an agent" } },
            { sessionUpdate: "subagent_spawned", subagentSessionId: "sub-1" },
            { sessionUpdate: "subagent_state_update", subagentSessionId: "sub-1", state: "completed" },
        ]);
        const ended = chatReducer(done, { type: "turn_completed", stopReason: "end_turn" });

        expect(ended.messages).toBe(done.messages);
        expect(ended.stopReason).toBe("end_turn");
    });

    it("cancels a finished subagent's call that was still running", () => {
        const done = replay(initialChatState, [{ sessionUpdate: "subagent_spawned", subagentSessionId: "sub-1" }]);
        const nested = update(done, { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "Grep", status: "in_progress" }, "sub-1");
        const completed = update(nested, { sessionUpdate: "subagent_state_update", subagentSessionId: "sub-1", state: "completed" });
        const ended = chatReducer(completed, { type: "turn_completed" });

        const part = ended.messages[0].parts[0];
        if (part.kind !== "subagent") throw new Error("expected a subagent");
        expect(part.subagent.state).toBe("completed");
        const inner = part.subagent.messages[0].parts[0];
        expect(inner.kind === "tool" && inner.tool.status).toBe("cancelled");
    });

    it("names an unnamed subagent and spawns each session once", () => {
        const spawned = update(initialChatState, { sessionUpdate: "subagent_spawned", subagentSessionId: "sub-1" });
        const again = update(spawned, { sessionUpdate: "subagent_spawned", subagentSessionId: "sub-1", name: "Other" });
        const anonymous = update(spawned, { sessionUpdate: "subagent_spawned" });

        const part = spawned.messages[0].parts[0];
        expect(part).toMatchObject({ kind: "subagent", subagent: { name: "Subagent", task: "", state: "running" } });
        expect(again).toBe(spawned);
        expect(anonymous).toBe(spawned);
    });

    it("ignores a subagent state it does not know or for a session it never saw", () => {
        const spawned = update(initialChatState, { sessionUpdate: "subagent_spawned", subagentSessionId: "sub-1" });

        expect(update(spawned, { sessionUpdate: "subagent_state_update", subagentSessionId: "sub-1", state: "exploded" })).toBe(spawned);
        expect(update(spawned, { sessionUpdate: "subagent_state_update", subagentSessionId: "sub-9", state: "completed" })).toBe(spawned);
        expect(update(spawned, { sessionUpdate: "subagent_state_update", state: "completed" })).toBe(spawned);
    });

    it("gives an unnamed background task defaults and starts each task once", () => {
        const spawned = update(initialChatState, { sessionUpdate: "async_task_spawned", asyncTaskId: "task-1" });

        expect(spawned.tasks).toEqual([
            {
                asyncTaskId: "task-1",
                name: "Background task",
                taskType: "",
                description: "",
                state: "running",
                canStop: false,
                outputFilePath: undefined,
            },
        ]);
        expect(update(spawned, { sessionUpdate: "async_task_spawned", asyncTaskId: "task-1" })).toBe(spawned);
        expect(update(spawned, { sessionUpdate: "async_task_spawned" })).toBe(spawned);
    });

    it("updates a paused task's details and ignores progress for an unknown task", () => {
        const spawned = update(initialChatState, { sessionUpdate: "async_task_spawned", asyncTaskId: "task-1", name: "Explore" });
        const paused = update(spawned, {
            sessionUpdate: "async_task_state_update",
            asyncTaskId: "task-1",
            state: "paused",
            description: "Searching",
            lastToolName: "Grep",
            outputFilePath: "/tmp/out.log",
            usage: { totalTokens: 10, toolUses: 2, durationMs: 300 },
        });
        const progressed = update(paused, { sessionUpdate: "async_task_progress", asyncTaskId: "task-1", state: "bogus" });

        expect(paused.tasks[0]).toMatchObject({
            state: "paused",
            description: "Searching",
            lastToolName: "Grep",
            outputFilePath: "/tmp/out.log",
            usage: { totalTokens: 10, toolUses: 2, durationMs: 300 },
        });
        expect(progressed.tasks[0]).toEqual(paused.tasks[0]);
        expect(update(paused, { sessionUpdate: "async_task_progress", asyncTaskId: "task-9", summary: "x" })).toBe(paused);
    });

    it("writes a notice without a summary when a stopped task never gave one", () => {
        const spawned = update(initialChatState, { sessionUpdate: "async_task_spawned", asyncTaskId: "task-1", name: "Build" });
        const stopped = update(spawned, { sessionUpdate: "async_task_state_update", asyncTaskId: "task-1", state: "stopped" });

        expect(stopped.tasks).toEqual([]);
        expect(stopped.messages[0].parts[0]).toEqual({ id: "notice-task-1", kind: "notice", notice: { name: "Build", state: "stopped" } });
    });

    it("keeps the plan, title and config the session reports", () => {
        const plan = { sessionUpdate: "plan", entries: [{ content: "Write tests", status: "pending" }] };
        const state = replay(initialChatState, [
            plan,
            { sessionUpdate: "session_info_update", title: "Coverage work" },
            { sessionUpdate: "config_option_update", configOptions: [{ id: "model" }] },
        ]);

        expect(state.plan).toEqual(plan);
        expect(state.title).toBe("Coverage work");
        expect(state.setup).toEqual({ configOptions: [{ id: "model" }] });
        expect(update(state, { sessionUpdate: "session_info_update", title: null }).title).toBe("Coverage work");
    });

    it("clears slash commands when the list is not an array and ignores unknown updates", () => {
        const withCommands = update(initialChatState, {
            sessionUpdate: "available_commands_update",
            availableCommands: [{ name: "help", description: "Help" }],
        });

        expect(update(withCommands, { sessionUpdate: "available_commands_update" }).commands).toEqual([]);
        expect(update(withCommands, { sessionUpdate: "something_new" })).toBe(withCommands);
    });

    it("keeps a usage cost only when both its amount and currency make sense", () => {
        const noCurrency = update(initialChatState, { sessionUpdate: "usage_update", used: 1, size: 10, cost: { amount: 2 } });
        const badAmount = update(initialChatState, { sessionUpdate: "usage_update", used: 1, size: 10, cost: { amount: Infinity, currency: "USD" } });

        expect(noCurrency.usage).toEqual({ used: 1, size: 10 });
        expect(badAmount.usage).toEqual({ used: 1, size: 10 });
        expect(update(initialChatState, { sessionUpdate: "usage_update", used: NaN, size: 10 }).usage).toBeNull();
    });

    it("uses saved usage only until the agent reports its own", () => {
        const saved = chatReducer(initialChatState, { type: "saved_usage", usage: { used: 5, size: 100 } });
        const reported = update(saved, { sessionUpdate: "usage_update", used: 50, size: 100 });

        expect(saved.usage).toEqual({ used: 5, size: 100 });
        expect(chatReducer(reported, { type: "saved_usage", usage: { used: 5, size: 100 } })).toBe(reported);
    });

    it("stores config options sent outside a session update", () => {
        const state = chatReducer(initialChatState, { type: "config", options: [{ id: "effort" }] });

        expect(state.setup).toEqual({ configOptions: [{ id: "effort" }] });
    });

    it("keeps a running turn through a starting status but ends it on an error", () => {
        const request = { requestId: "p1", sessionId: ROOT_SESSION, toolCall: { toolCallId: "t1", title: "Run" }, options: [] };
        const busy = replay(chatReducer(chatReducer(initialChatState, { type: "turn_started" }), { type: "permission_requested", request }), [
            { sessionUpdate: "async_task_spawned", asyncTaskId: "task-1" },
            { sessionUpdate: "tool_call", toolCallId: "t1", title: "Run", status: "in_progress" },
        ]);
        const errored = chatReducer({ ...busy, error: "boom" }, { type: "error", message: "boom" });
        const starting = chatReducer({ ...busy, error: "old" }, { type: "status", state: "starting" });
        const failed = chatReducer({ ...busy, error: "crashed" }, { type: "status", state: "error" });

        expect(starting).toMatchObject({ connection: "starting", running: true, error: null });
        expect(starting.permissions).toHaveLength(1);
        expect(starting.tasks).toHaveLength(1);
        expect(toolPart(starting).tool.status).toBe("in_progress");

        expect(failed).toMatchObject({ connection: "error", running: false, error: "crashed", permissions: [], tasks: [] });
        expect(toolPart(failed).tool.status).toBe("cancelled");
        expect(errored.connection).toBe("error");
    });

    it("keeps a ready connection ready when a prompt fails", () => {
        const ready = chatReducer(chatReducer(initialChatState, { type: "ready", capabilities: {}, setup: {} }), { type: "turn_started" });
        const failed = chatReducer(ready, { type: "error", message: "rate limited" });

        expect(failed).toMatchObject({ connection: "ready", running: false, error: "rate limited" });
    });

    it("sends a prompt of only attachments as a message without text", () => {
        const sent = chatReducer(initialChatState, { type: "local_prompt", text: "  ", paths: ["/tmp/shot.png"] });
        const plain = chatReducer(initialChatState, { type: "local_prompt", text: "Hi", paths: [] });

        expect(sent.messages[0]).toEqual({ id: "local-1", role: "user", sentAt: expect.any(Number), parts: [], attachments: ["/tmp/shot.png"] });
        expect(plain.messages[0]).not.toHaveProperty("attachments");
    });

    it("replaces a pending permission request that is asked again", () => {
        const request = { requestId: "p1", sessionId: ROOT_SESSION, toolCall: { toolCallId: "t1", title: "Run" }, options: [] };
        const renewed = { ...request, toolCall: { toolCallId: "t1", title: "Run again" } };
        const state = chatReducer(chatReducer(initialChatState, { type: "permission_requested", request }), {
            type: "permission_requested",
            request: renewed,
        });

        expect(state.permissions).toEqual([renewed]);
    });

    it("drops the held transcript and plan when the first replayed update lands", () => {
        const planned = update(initialChatState, { sessionUpdate: "plan", entries: [] });
        const held = chatReducer({ ...planned, messages: [{ id: "m1", role: "assistant", parts: [] }] }, { type: "reset", hold: true });
        const resumed = update(held, { sessionUpdate: "usage_update", used: 1, size: 10 });

        expect(held.awaitingReplay).toBe(true);
        expect(resumed).toMatchObject({ awaitingReplay: false, messages: [], plan: null });
        expect(chatReducer(initialChatState, { type: "reset", hold: true }).awaitingReplay).toBe(false);
    });

    it("cancels a running subagent that had only written text when the turn ends", () => {
        const spawned = update(initialChatState, { sessionUpdate: "subagent_spawned", subagentSessionId: "sub-1" });
        const wrote = update(spawned, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Looking" } }, "sub-1");
        const ended = chatReducer(wrote, { type: "turn_completed" });

        const before = wrote.messages[0].parts[0];
        const part = ended.messages[0].parts[0];
        if (part.kind !== "subagent" || before.kind !== "subagent") throw new Error("expected a subagent");
        expect(part.subagent.state).toBe("cancelled");
        expect(part.subagent.messages).toBe(before.subagent.messages);
    });

    it("keeps a subagent's transcript as is when its session sends an update it cannot place", () => {
        const spawned = update(initialChatState, { sessionUpdate: "subagent_spawned", subagentSessionId: "sub-1" });
        const orphan = update(spawned, { sessionUpdate: "tool_call_update", toolCallId: "tool-9", status: "completed" }, "sub-1");

        expect(orphan.messages[0].parts).toEqual(spawned.messages[0].parts);
    });
});
