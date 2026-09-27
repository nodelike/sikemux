import { describe, expect, it } from "vitest";
import { caretAtEdge, recallPrompt, sentPrompts } from "./promptHistory";
import type { ChatMessage } from "./types";

const said = (role: ChatMessage["role"], text: string): ChatMessage => ({ id: text, role, parts: [{ id: `${text}-t`, kind: "text", text }] });

describe("the messages ↑ can bring back", () => {
    it("are what was sent, oldest first, then anything still queued", () => {
        const messages = [said("user", "fix the build"), said("assistant", "done"), said("user", " run the tests ")];
        expect(sentPrompts(messages, ["and push"])).toEqual(["fix the build", "run the tests", "and push"]);
    });

    it("keep a message sent twice in a row once, and skip one that was only files", () => {
        const onlyFiles: ChatMessage = { id: "f", role: "user", parts: [], attachments: ["/a.png"] };
        expect(sentPrompts([said("user", "again"), said("user", "again"), onlyFiles])).toEqual(["again"]);
    });
});

describe("stepping through them", () => {
    const prompts = ["first", "second", "third"];

    it("goes older from the newest and stops at the oldest", () => {
        const one = recallPrompt(prompts, null, "half typed", "older");
        expect(one).toEqual({ draft: "third", position: { index: 2, typed: "half typed" } });
        const two = recallPrompt(prompts, one!.position, one!.draft, "older");
        const three = recallPrompt(prompts, two!.position, two!.draft, "older");
        expect(three?.draft).toBe("first");
        expect(recallPrompt(prompts, three!.position, three!.draft, "older")).toBeNull();
    });

    it("goes newer and ends back on what was being typed", () => {
        const second = { index: 1, typed: "half typed" };
        expect(recallPrompt(prompts, second, "second", "newer")?.draft).toBe("third");
        expect(recallPrompt(prompts, { index: 2, typed: "half typed" }, "third", "newer")).toEqual({ draft: "half typed", position: null });
    });

    it("does nothing going newer before browsing, or older with nothing sent", () => {
        expect(recallPrompt(prompts, null, "x", "newer")).toBeNull();
        expect(recallPrompt([], null, "x", "older")).toBeNull();
    });
});

describe("when the arrows browse instead of moving the caret", () => {
    it("only from the first line going up and the last line going down", () => {
        const text = "one\ntwo";
        expect(caretAtEdge(text, 2, 2, "older")).toBe(true);
        expect(caretAtEdge(text, 6, 6, "older")).toBe(false);
        expect(caretAtEdge(text, 6, 6, "newer")).toBe(true);
        expect(caretAtEdge(text, 2, 2, "newer")).toBe(false);
    });

    it("never while text is selected", () => {
        expect(caretAtEdge("one", 0, 3, "older")).toBe(false);
    });
});
