import { describe, expect, it } from "vitest";
import { activeToolLabel, activityLabel, toolKind, toolLabel, toolPath, toolRunning, toolTarget, toolUrl } from "./toolLabels";
import type { AcpToolCall, ChatMessage, ChatPart } from "./types";

const call = (title: string, extra: Partial<AcpToolCall> = {}): AcpToolCall => ({ toolCallId: "t1", title, ...extra });

describe("toolLabel", () => {
    it("splits an MCP tool into its server and name", () => {
        expect(toolLabel("mcp__github__list_pulls")).toEqual({ scope: "github", name: "list_pulls" });
        expect(toolLabel("mcp__server__nested__tool")).toEqual({ scope: "server", name: "nested__tool" });
    });

    it("leaves anything else whole", () => {
        expect(toolLabel("mcp__half")).toEqual({ name: "mcp__half" });
        expect(toolLabel("Read file")).toEqual({ name: "Read file" });
    });
});

describe("activityLabel", () => {
    it("says what a known kind is doing", () => {
        expect(activityLabel(call("rm -rf node_modules && pnpm i", { kind: "execute" }))).toBe("Running a command…");
        expect(activityLabel(call("x", { kind: "switch_mode" }))).toBe("Switching mode…");
    });

    it("uses the first line of a short title for an unknown kind", () => {
        expect(activityLabel(call("mcp__github__list_pulls\nmore"))).toBe("list_pulls");
    });

    it("falls back when the title is empty or too long", () => {
        expect(activityLabel(call("   "))).toBe("Working…");
        expect(activityLabel(call("a".repeat(41)))).toBe("Working…");
        expect(activityLabel(call("a".repeat(40)))).toBe("a".repeat(40));
    });
});

describe("toolKind", () => {
    it("uses the word for a known kind", () => {
        expect(toolKind(call("cargo test", { kind: "execute" }))).toBe("run");
        expect(toolKind(call("x", { kind: "switch_mode" }))).toBe("mode");
    });

    it("names an MCP call for its server", () => {
        expect(toolKind(call("mcp__github__list_pulls", { kind: "fetch_custom" }))).toBe("github");
    });

    it("cuts an unknown title down to its first word", () => {
        expect(toolKind(call("WebSearch(query)"))).toBe("websearch");
        expect(toolKind(call("Averyveryverylongtoolname"))).toBe("averyveryver");
    });
});

describe("toolTarget", () => {
    it("shortens a path to the name it ends in", () => {
        expect(toolTarget(call("/work/demo/src/main.rs"))).toBe("main.rs");
    });

    it("keeps a command, a URL and a word as written", () => {
        expect(toolTarget(call("cat src/main.rs"))).toBe("cat src/main.rs");
        expect(toolTarget(call("https://example.com/a/b"))).toBe("https://example.com/a/b");
        expect(toolTarget(call("Read"))).toBe("Read");
    });

    it("reads only the first line", () => {
        expect(toolTarget(call("  src/lib.rs  \nsecond"))).toBe("lib.rs");
    });

    it("keeps a path that has no name after its last slash", () => {
        expect(toolTarget(call("/"))).toBe("/");
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

describe("toolPath", () => {
    it("prefers the location the call reported, with its line", () => {
        expect(toolPath(call("Read", { locations: [{ path: "/a/b.ts", line: 12 }] }))).toBe("/a/b.ts:12");
        expect(toolPath(call("Read", { locations: [{ path: "/a/b.ts" }] }))).toBe("/a/b.ts");
    });

    it("falls back to a path in the title", () => {
        expect(toolPath(call("src/app.tsx", { locations: [{ path: "" }] }))).toBe("src/app.tsx");
        expect(toolPath(call("src/app.tsx", { locations: ["junk"] }))).toBe("src/app.tsx");
    });

    it("does not read a command, a URL or a bare word as a file", () => {
        expect(toolPath(call("cat src/app.tsx"))).toBeNull();
        expect(toolPath(call("https://example.com/x"))).toBeNull();
        expect(toolPath(call("Read"))).toBeNull();
    });
});

describe("toolRunning", () => {
    it("counts a call as running until it has an ending status", () => {
        expect(toolRunning(call("x"))).toBe(true);
        expect(toolRunning(call("x", { status: "in_progress" }))).toBe(true);
        for (const status of ["completed", "failed", "cancelled"]) expect(toolRunning(call("x", { status }))).toBe(false);
    });
});

describe("activeToolLabel", () => {
    const tool = (id: string, extra: Partial<AcpToolCall>): ChatPart => ({ id, kind: "tool", tool: call("x", { toolCallId: id, ...extra }) });
    const message = (parts: ChatPart[]): ChatMessage => ({ id: "m", role: "assistant", parts });

    it("names what the last call in the last message is doing", () => {
        const parts = [tool("a", { status: "completed" }), tool("b", { kind: "read" }), { id: "t", kind: "text", text: "hi" } as ChatPart];
        expect(activeToolLabel([message(parts)])).toBe("Reading…");
    });

    it("says nothing once the last call has finished", () => {
        expect(activeToolLabel([message([tool("a", { kind: "read" }), tool("b", { status: "completed" })])])).toBeNull();
    });

    it("says nothing without calls or messages", () => {
        expect(activeToolLabel([message([{ id: "t", kind: "text", text: "hi" }])])).toBeNull();
        expect(activeToolLabel([])).toBeNull();
    });
});
