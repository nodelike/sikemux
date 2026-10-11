import { describe, expect, it } from "vitest";
import { activeToolLabel, activityLabel, toolDetail, toolKind, toolLabel, toolPath, toolRowArguments, toolTarget, toolUrl } from "./toolLabels";
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
    it("uses the first line of a short title for an unknown kind", () => {
        expect(activityLabel(call("mcp__github__list_pulls\nmore"))).toBe("list pulls");
    });

    it("falls back when the title is empty or too long", () => {
        expect(activityLabel(call("   "))).toBe("Working…");
        expect(activityLabel(call("a".repeat(41)))).toBe("Working…");
        expect(activityLabel(call("a".repeat(40)))).toBe("a".repeat(40));
    });
});

describe("toolKind", () => {
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

describe("our own tools", () => {
    const navigate = (rawInput: unknown, title = "mcp__sikemux-tools__browser_navigate") => call(title, { kind: "other", rawInput });

    it("say what they did to what, in place of the server and function name", () => {
        const tool = navigate({ url: "https://github.com/nodelike/sikemux" });
        expect(toolKind(tool)).toBe("open");
        expect(toolTarget(tool)).toBe("https://github.com/nodelike/sikemux");
        expect(toolDetail(tool)).toBeNull();
    });

    it("take the first template the call has every argument for", () => {
        expect(toolTarget(navigate({ go: "back" }))).toBe("back");
        const click = (rawInput: unknown) => call("mcp__sikemux-tools__browser_click", { rawInput });
        expect(toolTarget(click({ text: "Sign in", role: "button" }))).toBe("“Sign in”");
        expect(toolDetail(click({ text: "Sign in", role: "button" }))).toBe("button");
        expect(toolTarget(click({ index: 0 }))).toBe("element 0");
        expect(toolTarget(click({}))).toBe("");
    });

    it("are found however the agent writes the server into the title", () => {
        for (const title of ["sikemux-tools.browser_navigate", "sikemux-tools/browser_navigate", "browser_navigate"]) {
            expect(toolKind(navigate({ url: "https://a.dev" }, title))).toBe("open");
        }
        expect(toolTarget(navigate({ server: "sikemux-tools", tool: "browser_navigate", arguments: { url: "https://a.dev" } }))).toBe(
            "https://a.dev",
        );
    });

    it("never treat their title as a file", () => {
        expect(toolPath(navigate({ url: "https://a.dev" }, "sikemux-tools/browser_navigate"))).toBeNull();
    });

    it("show a path by its name, a list joined and a list of steps by its count", () => {
        expect(toolTarget(call("mcp__sikemux-tools__browser_upload", { rawInput: { paths: ["/tmp/a.png", "/tmp/b.pdf"] } }))).toBe("a.png, b.pdf");
        expect(toolTarget(call("mcp__sikemux-tools__browser_act", { rawInput: { steps: [{ action: "click" }, { action: "press" }] } }))).toBe(
            "2-step sequence",
        );
    });

    it("say what they are doing while they run", () => {
        expect(activityLabel(navigate({ url: "https://a.dev" }))).toBe("open https://a.dev");
        expect(activityLabel(navigate({ url: `https://a.dev/${"x".repeat(40)}` }))).toBe("open");
    });

    it("draw the same row from the arguments a finished call keeps", () => {
        const live = call("mcp__sikemux-tools__browser_upload", { rawInput: { paths: ["/tmp/a.png"], index: 3, report: "full" } });
        const kept = call(live.title, { rawInput: toolRowArguments(live) });
        expect([toolTarget(kept), toolDetail(kept)]).toEqual([toolTarget(live), toolDetail(live)]);
        const act = call("mcp__sikemux-tools__browser_act", { rawInput: { steps: [{ action: "click" }, { action: "press" }] } });
        expect(toolTarget(call(act.title, { rawInput: toolRowArguments(act) }))).toBe("2-step sequence");
    });

    it("say nothing rather than fill in a default, and give waits as durations", () => {
        expect(toolTarget(call("mcp__sikemux-tools__browser_screenshot", { rawInput: {} }))).toBe("");
        expect(toolTarget(call("mcp__sikemux-tools__browser_wait", { rawInput: { ms: 4000 } }))).toBe("4s");
    });

    it("are drawn for plugin tools too", () => {
        const logs = call("mcp__sikemux-tools__signoz_logs", { rawInput: { service: "api", text: "timeout" } });
        expect([toolKind(logs), toolTarget(logs), toolDetail(logs)]).toEqual(["logs", "api", "“timeout”"]);
    });
});

describe("another server's tools", () => {
    it("read their name as words and show the first short argument", () => {
        const tool = call("mcp__linear__create_issue", { rawInput: { title: "Rows use underscores", body: "a\nb" } });
        expect(toolKind(tool)).toBe("linear");
        expect(toolTarget(tool)).toBe("create issue");
        expect(toolDetail(tool)).toBe("Rows use underscores");
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
