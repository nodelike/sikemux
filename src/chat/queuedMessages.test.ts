import { describe, expect, it } from "vitest";
import { queuedLabel } from "./queuedMessages";
import type { QueuedMessage } from "./types";

const queued = (id: string, text: string, paths: string[] = [], context: QueuedMessage["context"] = []): QueuedMessage => ({
    id,
    text,
    paths,
    context,
});
const item = (uri: string) => ({ uri, title: uri, text: "body" });

describe("queuedLabel", () => {
    it("names context items when there is no text", () => {
        expect(queuedLabel(queued("1", "", ["/a/b.png"], [item("#12 Crash")]))).toBe("b.png, #12 Crash");
    });
});
