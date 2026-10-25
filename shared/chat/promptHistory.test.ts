import { describe, expect, it } from "vitest";
import { arrowsBrowse, recallPrompt, sentPrompts } from "./promptHistory";
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

    it("read only the words of a message that also carried a picture", () => {
        const withPicture: ChatMessage = {
            id: "p",
            role: "user",
            parts: [
                { id: "p-c", kind: "content", content: { type: "image", data: "AAAA", mimeType: "image/png" } },
                { id: "p-t", kind: "text", text: "what is this" },
            ],
        };
        expect(sentPrompts([withPicture])).toEqual(["what is this"]);
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
    const prompts = ["first", "second"];

    it("goes up into the sent messages only from an empty composer", () => {
        expect(arrowsBrowse("", prompts, null, "older")).toBe(true);
        expect(arrowsBrowse("  \n ", prompts, null, "older")).toBe(true);
        expect(arrowsBrowse("", prompts, null, "newer")).toBe(false);
    });

    it("leaves the arrows to the text once anything is typed, on any line", () => {
        expect(arrowsBrowse("a long line that wraps", prompts, null, "older")).toBe(false);
        expect(arrowsBrowse("one\ntwo\nthree", prompts, null, "older")).toBe(false);
    });

    it("keeps browsing while a recalled message is untouched, and stops once it is edited", () => {
        expect(arrowsBrowse("second", prompts, { index: 1, typed: "" }, "older")).toBe(true);
        expect(arrowsBrowse("second", prompts, { index: 1, typed: "" }, "newer")).toBe(true);
        expect(arrowsBrowse("second, but changed", prompts, { index: 1, typed: "" }, "older")).toBe(false);
    });
});
