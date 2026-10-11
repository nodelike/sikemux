import { describe, expect, it } from "vitest";
import { readPageMessage, toolPage } from "./pages";
import type { AcpToolCall } from "./types";

const id = "0123456789abcdef0123456789abcdef";
const shown = (overrides: Partial<AcpToolCall>): AcpToolCall => ({
    toolCallId: "t1",
    title: "mcp__sikemux-tools__page_show",
    status: "completed",
    rawOutput: JSON.stringify({ page: { id, title: "Revenue", height: 412 }, note: "shown" }),
    ...overrides,
});

describe("toolPage", () => {
    it("reads the page a finished page_show printed", () => {
        expect(toolPage(shown({}))).toEqual({ id, title: "Revenue", height: 412 });
    });

    it("reads it from MCP blocks and from content when the host keeps it there", () => {
        const text = JSON.stringify({ page: { id, title: "Revenue" } });
        expect(toolPage(shown({ rawOutput: { result: { content: [{ type: "text", text }] } } }))).toEqual({ id, title: "Revenue" });
        expect(toolPage(shown({ rawOutput: undefined, content: [{ type: "content", content: { type: "text", text } }] }))).toEqual({
            id,
            title: "Revenue",
        });
    });

    it("finds a page under any host's way of naming the tool", () => {
        expect(toolPage(shown({ title: "sikemux-tools.page_show" }))).not.toBeNull();
    });

    it("ignores other tools, unfinished calls and anything that is not a page id", () => {
        expect(toolPage(shown({ title: "mcp__sikemux-tools__ui_open" }))).toBeNull();
        expect(toolPage(shown({ status: "in_progress" }))).toBeNull();
        expect(toolPage(shown({ rawOutput: JSON.stringify({ page: { id: "../../etc/passwd", title: "x" } }) }))).toBeNull();
        expect(toolPage(shown({ rawOutput: "path must be absolute" }))).toBeNull();
    });
});

describe("readPageMessage", () => {
    it("takes a size report and a web link", () => {
        expect(readPageMessage({ jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: 230 } })).toEqual({
            kind: "height",
            height: 230,
        });
        expect(readPageMessage({ jsonrpc: "2.0", id: "l1", method: "ui/open-link", params: { url: "https://a.test/x" } })).toEqual({
            kind: "link",
            url: "https://a.test/x",
        });
    });

    it("takes a wheel the page could not use", () => {
        expect(readPageMessage({ jsonrpc: "2.0", method: "sikemux/wheel", params: { deltaY: -48 } })).toEqual({ kind: "wheel", deltaY: -48 });
        expect(readPageMessage({ jsonrpc: "2.0", method: "sikemux/wheel", params: { deltaY: "far" } })).toBeNull();
    });

    it("refuses links that are not to the web and anything malformed", () => {
        expect(readPageMessage({ jsonrpc: "2.0", method: "ui/open-link", params: { url: "file:///etc/passwd" } })).toBeNull();
        expect(readPageMessage({ jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: -1 } })).toBeNull();
        expect(readPageMessage({ method: "ui/notifications/size-changed", params: { height: 10 } })).toBeNull();
        expect(readPageMessage("hello")).toBeNull();
    });
});
