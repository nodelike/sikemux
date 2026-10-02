import { describe, expect, it } from "vitest";
import { combineQueued, nextBatch, queuedLabel, type QueuedMessage } from "./queuedMessages";

const queued = (id: string, text: string, paths: string[] = [], context: QueuedMessage["context"] = []): QueuedMessage => ({
    id,
    text,
    paths,
    context,
});
const item = (uri: string) => ({ uri, title: uri, text: "body" });

describe("queuedLabel", () => {
    it("shows the text when there is some", () => {
        expect(queuedLabel(queued("1", "fix it", ["/a/b.png"]))).toBe("fix it");
    });

    it("names the attachments when there is no text", () => {
        expect(queuedLabel(queued("1", "", ["/a/b.png", "/c/d.txt"]))).toBe("b.png, d.txt");
    });

    it("names context items when there is no text", () => {
        expect(queuedLabel(queued("1", "", ["/a/b.png"], [item("#12 Crash")]))).toBe("b.png, #12 Crash");
    });
});

describe("nextBatch", () => {
    it("sends nothing from an empty queue", () => {
        expect(nextBatch([])).toEqual([]);
    });

    it("sends every plain message together", () => {
        const messages = [queued("1", "a"), queued("2", "b")];
        expect(nextBatch(messages)).toEqual(messages);
    });

    it("sends a leading command on its own", () => {
        const messages = [queued("1", "/compact"), queued("2", "b")];
        expect(nextBatch(messages)).toEqual([messages[0]]);
    });

    it("stops the batch before the next command", () => {
        const messages = [queued("1", "a"), queued("2", "b"), queued("3", "/review"), queued("4", "c")];
        expect(nextBatch(messages)).toEqual(messages.slice(0, 2));
    });
});

describe("combineQueued", () => {
    it("joins the texts and keeps the first id", () => {
        expect(combineQueued([queued("1", "a"), queued("2", ""), queued("3", "c")])).toEqual({ id: "1", text: "a\n\nc", paths: [], context: [] });
    });

    it("merges attachments without repeating one", () => {
        expect(combineQueued([queued("1", "a", ["/x", "/y"]), queued("2", "b", ["/y", "/z"])]).paths).toEqual(["/x", "/y", "/z"]);
    });

    it("merges context items without repeating one", () => {
        const combined = combineQueued([queued("1", "a", [], [item("u1")]), queued("2", "b", [], [item("u1"), item("u2")])]);
        expect(combined.context.map((entry) => entry.uri)).toEqual(["u1", "u2"]);
    });
});
