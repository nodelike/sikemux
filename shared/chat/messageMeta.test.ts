import { expect, it } from "vitest";
import { copyText, rateLabel, sentLabel, sentTitle } from "./messageMeta";
import type { ChatMessage, ChatPart } from "./types";

const text = (id: string, value: string): ChatPart => ({ kind: "text", id, text: value });
const toolPart = (id: string, title: string): ChatPart => ({
    kind: "tool",
    id,
    tool: { toolCallId: id, title, status: "completed", row: { action: "other", target: title } },
});

it("spells a slow rate out to a decimal and a fast one whole", () => {
    expect(rateLabel(4.27)).toBe("~4.3 tok/s");
    expect(rateLabel(63.4)).toBe("~63 tok/s");
});

it("copies the prose and the files a turn was sent with, not the tool rows", () => {
    const messages: ChatMessage[] = [
        { id: "u1", role: "user", parts: [text("u1-t", "check the build")], attachments: ["/tmp/log.txt"], meta: { from: "u1" } },
        { id: "m1", role: "assistant", parts: [toolPart("t1", "cargo check"), text("m1-t", "It builds clean.")], meta: { from: "m1" } },
    ];

    expect(copyText(messages, 0)).toBe("check the build\n\n/tmp/log.txt");
    expect(copyText(messages, 1)).toBe("It builds clean.");
});

it("copies a whole answer from where it ends, and offers nothing in the middle of one", () => {
    const messages: ChatMessage[] = [
        { id: "u1", role: "user", parts: [text("u1-t", "wire it up")] },
        { id: "m1", role: "assistant", parts: [text("m1-t", "Looking now.")] },
        { id: "m2", role: "assistant", parts: [toolPart("t1", "rg wire")] },
        { id: "m3", role: "assistant", parts: [text("m3-t", "Wired.")], meta: { from: "m1" } },
    ];

    expect(copyText(messages, 1)).toBe("");
    expect(copyText(messages, 3)).toBe("Looking now.\n\nWired.");
});

it("says when with the day, as near as it is", () => {
    const now = new Date(2026, 8, 29, 23, 30).getTime();
    expect(sentLabel(new Date(2026, 8, 29, 21, 7).getTime(), now, "en-GB")).toBe("Today, 21:07");
    expect(sentLabel(new Date(2026, 8, 28, 9, 5).getTime(), now, "en-GB")).toBe("Yesterday, 09:05");
    expect(sentLabel(new Date(2026, 8, 3, 14, 2).getTime(), now, "en-GB")).toBe("3 Sept, 14:02");
    expect(sentLabel(new Date(2025, 11, 31, 18, 45).getTime(), now, "en-GB")).toBe("31 Dec 2025, 18:45");
    expect(sentTitle(new Date(2026, 8, 29, 21, 7, 15).getTime(), "en-GB")).toBe("Tuesday, 29 September 2026 at 21:07:15");
});
