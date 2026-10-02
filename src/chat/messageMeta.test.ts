import { expect, it, vi } from "vitest";
import { chatReducer, initialChatState } from "./reducer";
import { rateLabel, rowMeta, sentLabel, sentTitle } from "./messageMeta";
import type { ChatState } from "./types";

const update = (state: ChatState, value: Record<string, unknown>) =>
    chatReducer(state, { type: "session_update", sessionId: "session-1", update: value });

const chunk = (text: string, messageId = "m1") => ({
    sessionUpdate: "agent_message_chunk",
    messageId,
    content: { type: "text", text },
});

const last = (state: ChatState) => rowMeta(state.messages, state.messages.length - 1);

it("reads the rate off a turn it watched stream", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-18T00:00:00Z"));
    let state = chatReducer(initialChatState, { type: "turn_started" });
    state = update(state, chunk("a".repeat(40)));
    vi.setSystemTime(new Date("2026-09-18T00:00:02Z"));
    state = update(state, chunk("b".repeat(40)));

    // 80 characters is ~20 tokens, written over two seconds.
    expect(last(state).rate).toBeCloseTo(10);
    vi.useRealTimers();
});

it("times the writing and not the tool call waited on in the middle of it", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-18T00:00:00Z"));
    let state = chatReducer(initialChatState, { type: "turn_started" });
    state = update(state, chunk("a".repeat(40), "m1"));
    vi.setSystemTime(new Date("2026-09-18T00:00:01Z"));
    state = update(state, chunk("b".repeat(40), "m1"));

    vi.setSystemTime(new Date("2026-09-18T00:01:00Z"));
    state = update(state, { sessionUpdate: "tool_call", toolCallId: "t1", title: "cargo check", status: "completed" });
    state = update(state, chunk("c".repeat(40), "m2"));
    vi.setSystemTime(new Date("2026-09-18T00:01:01Z"));
    state = update(state, chunk("d".repeat(40), "m2"));

    // 160 characters is ~40 tokens, written over the two seconds either side of
    // a minute-long tool call.
    expect(last(state).rate).toBeCloseTo(20);
    vi.useRealTimers();
});

it("times nothing for history replayed after a reconnect", () => {
    const state = update(initialChatState, chunk("a".repeat(400)));
    expect(state.messages[0].streamStartedAt).toBeUndefined();
    expect(last(state).rate).toBeNull();
});

it("gives no rate for a burst too short to measure", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-18T00:00:00Z"));
    let state = chatReducer(initialChatState, { type: "turn_started" });
    state = update(state, chunk("hello"));
    vi.setSystemTime(new Date("2026-09-18T00:00:00.100Z"));
    state = update(state, chunk(" there"));

    expect(last(state).rate).toBeNull();
    vi.useRealTimers();
});

it("spells a slow rate out to a decimal and a fast one whole", () => {
    expect(rateLabel(4.27)).toBe("~4.3 tok/s");
    expect(rateLabel(63.4)).toBe("~63 tok/s");
});

it("copies the prose and the files a turn was sent with, not the tool rows", () => {
    const state = update(
        update(chatReducer(initialChatState, { type: "local_prompt", text: "check the build", paths: ["/tmp/log.txt"] }), {
            sessionUpdate: "tool_call",
            toolCallId: "t1",
            title: "cargo check",
            status: "pending",
        }),
        chunk("It builds clean."),
    );

    expect(rowMeta(state.messages, 0).text).toBe("check the build\n\n/tmp/log.txt");
    expect(last(state).text).toBe("It builds clean.");
});

it("copies a whole answer from where it ends, and offers nothing in the middle of one", () => {
    let state = update(chatReducer(initialChatState, { type: "local_prompt", text: "wire it up", paths: [] }), chunk("Looking now.", "m1"));
    state = update(state, { sessionUpdate: "tool_call", toolCallId: "t1", title: "rg wire", status: "completed" });
    state = update(state, chunk("Wired.", "m2"));

    const middle = state.messages.findIndex((message) => message.id === "m1");
    expect(rowMeta(state.messages, middle).text).toBe("");
    expect(last(state).text).toBe("Looking now.\n\nWired.");
});

it("counts no characters for a streamed message that never said how many arrived", () => {
    const messages = [
        {
            id: "m1",
            role: "assistant" as const,
            parts: [{ id: "m1-t", kind: "text" as const, text: "hello" }],
            streamStartedAt: 0,
            streamEndedAt: 1_000,
        },
    ];
    expect(rowMeta(messages, 0)).toEqual({ text: "hello", rate: null, at: null, took: null });
});

it("stamps a prompt with when it was sent, and its answer with when the turn finished and how long it took", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 29, 21, 7, 0));
    let state = chatReducer(initialChatState, { type: "local_prompt", text: "check the build", paths: [] });
    state = chatReducer(state, { type: "turn_started" });
    vi.setSystemTime(new Date(2026, 8, 29, 21, 7, 4));
    state = update(state, chunk("Looking.", "m1"));
    vi.setSystemTime(new Date(2026, 8, 29, 21, 9, 0));
    state = update(state, { sessionUpdate: "tool_call", toolCallId: "t1", title: "cargo check", status: "completed" });
    state = update(state, chunk("It builds.", "m2"));
    expect(last(state).at).toBeNull();

    vi.setSystemTime(new Date(2026, 8, 29, 21, 11, 52));
    state = chatReducer(state, { type: "turn_completed" });

    expect(rowMeta(state.messages, 0)).toMatchObject({ at: new Date(2026, 8, 29, 21, 7, 0).getTime(), took: null });
    expect(last(state)).toMatchObject({ at: new Date(2026, 8, 29, 21, 11, 52).getTime(), took: (4 * 60 + 52) * 1000 });
    vi.useRealTimers();
});

it("has no time for history replayed after a reconnect", () => {
    let state = update(initialChatState, chunk("from before"));
    state = chatReducer(state, { type: "turn_completed" });
    expect(state.messages[0].sentAt).toBeUndefined();
    expect(state.messages[0].endedAt).toBeUndefined();
    expect(last(state)).toMatchObject({ at: null, took: null });
});

it("says when with the day, as near as it is", () => {
    const now = new Date(2026, 8, 29, 23, 30).getTime();
    expect(sentLabel(new Date(2026, 8, 29, 21, 7).getTime(), now, "en-GB")).toBe("Today, 21:07");
    expect(sentLabel(new Date(2026, 8, 28, 9, 5).getTime(), now, "en-GB")).toBe("Yesterday, 09:05");
    expect(sentLabel(new Date(2026, 8, 3, 14, 2).getTime(), now, "en-GB")).toBe("3 Sept, 14:02");
    expect(sentLabel(new Date(2025, 11, 31, 18, 45).getTime(), now, "en-GB")).toBe("31 Dec 2025, 18:45");
    expect(sentTitle(new Date(2026, 8, 29, 21, 7, 15).getTime(), "en-GB")).toBe("Tuesday, 29 September 2026 at 21:07:15");
});
