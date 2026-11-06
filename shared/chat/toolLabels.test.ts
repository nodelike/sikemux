import { describe, expect, it } from "vitest";
import { activityLabel, toolKind, toolUrl } from "./toolLabels";
import type { ToolCall, ChatMessage, ChatPart } from "./types";

const call = (title: string, extra: Partial<ToolCall> = {}): ToolCall => ({
    toolCallId: "t1",
    title,
    row: { action: "other", target: title },
    ...extra,
});

describe("a row's words", () => {
    it("take the call's own word, then its server, then the app's word for what it did", () => {
        expect(toolKind(call("x", { row: { action: "fetch", target: "x", verb: "open" } }))).toBe("open");
        expect(toolKind(call("x", { row: { action: "mcp", target: "x", server: "github" } }))).toBe("github");
        expect(toolKind(call("x", { row: { action: "execute", target: "x" } }))).toBe("run");
    });

    it("say what a running call is doing, in the core's words when it sends some", () => {
        expect(activityLabel({ action: "fetch", target: "x", activity: "open https://a.dev" })).toBe("open https://a.dev");
        expect(activityLabel({ action: "execute", target: "x" })).toBe("Running a command…");
        expect(activityLabel({ action: "other", target: "x" })).toBe("Working…");
    });
});

describe("toolUrl", () => {
    it("finds a URL and the text either side of it", () => {
        expect(toolUrl("fetch https://example.com/page now")).toEqual({
            before: "fetch ",
            raw: "https://example.com/page",
            url: "https://example.com/page",
            after: " now",
        });
    });

    it("leaves trailing punctuation outside the link", () => {
        const link = toolUrl("see (https://example.com/a).");
        expect(link?.raw).toBe("https://example.com/a");
        expect(link?.after).toBe(").");
    });

    it("finds nothing without a web URL", () => {
        expect(toolUrl("cargo test")).toBeNull();
        expect(toolUrl("https://user:pw@example.com")).toBeNull();
    });
});
