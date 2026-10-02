import { describe, expect, it } from "vitest";
import { findMatches, findPattern, messageSearchText, rangesIn } from "./chatSearch";
import type { ChatMessage } from "./types";

const plain = { caseSensitive: false, wholeWord: false };
const said = (role: ChatMessage["role"], text: string): ChatMessage => ({ id: text, role, parts: [{ id: `${text}-t`, kind: "text", text }] });

describe("the pattern a query becomes", () => {
    it("matches the query as typed, including characters a pattern would read specially", () => {
        expect("cost is $5.00 (net)".match(findPattern("$5.00 (net)", plain)!)).toEqual(["$5.00 (net)"]);
    });

    it("ignores case unless asked, and can hold to whole words", () => {
        expect("Style styles STYLE".match(findPattern("style", plain)!)).toHaveLength(3);
        expect("Style styles STYLE".match(findPattern("style", { ...plain, caseSensitive: true })!)).toEqual(["style"]);
        expect("Style styles STYLE".match(findPattern("style", { ...plain, wholeWord: true })!)).toEqual(["Style", "STYLE"]);
    });

    it("is nothing while the query is empty", () => {
        expect(findPattern("", plain)).toBeNull();
        expect(findMatches([said("user", "anything")], null)).toEqual([]);
    });
});

describe("finding matches across a conversation", () => {
    it("lists every occurrence, message by message", () => {
        const messages = [said("user", "Look at the styles"), said("assistant", "The styles are in chat.css; the styles are tidy")];

        expect(findMatches(messages, findPattern("styles", plain))).toEqual([
            { message: 0, occurrence: 0 },
            { message: 1, occurrence: 0 },
            { message: 1, occurrence: 1 },
        ]);
    });

    it("reads a message's prose, not the tool calls it made", () => {
        const message: ChatMessage = {
            id: "m",
            role: "assistant",
            parts: [
                { id: "t", kind: "text", text: "Checked it." },
                { id: "c", kind: "tool", tool: { toolCallId: "1", title: "grep styles", kind: "execute", status: "completed" } as never },
            ],
        };
        expect(messageSearchText(message)).toBe("Checked it.\n");
    });
});

describe("marking matches in what is on screen", () => {
    it("keeps a match that crosses bold or code as one range", () => {
        const root = document.createElement("div");
        root.innerHTML = "Use <strong>Find</strong> email, then <code>Find email</code>.";

        const ranges = rangesIn(root, findPattern("find email", plain)!);

        expect(ranges.map((range) => range.toString())).toEqual(["Find email", "Find email"]);
        expect(ranges[0].startContainer.textContent).toBe("Find");
        expect(ranges[0].endContainer.textContent).toBe(" email, then ");
    });
});
