import { describe, expect, it } from "vitest";
import { toolDescription, toolOutput } from "./toolOutput";
import type { AcpToolCall } from "./types";

const call = (fields: Partial<AcpToolCall>): AcpToolCall => ({ toolCallId: "tool-1", title: "ls", kind: "execute", status: "completed", ...fields });

describe("toolOutput", () => {
    it("reads Claude's result whether it arrives as a string or as blocks", () => {
        expect(toolOutput(call({ rawOutput: "a\nb\n" }))).toEqual({ text: "a\nb", cut: false });
        expect(
            toolOutput(
                call({
                    rawOutput: [
                        { type: "text", text: "one" },
                        { type: "text", text: "two" },
                    ],
                }),
            )?.text,
        ).toBe("one\ntwo");
    });

    it("reads Codex's formatted output and an MCP call's result", () => {
        expect(toolOutput(call({ rawOutput: { formatted_output: "done", exit_code: 0 } }))).toEqual({ text: "done", cut: false, exitCode: 0 });
        const mcp = call({ title: "mcp.docs.search", rawOutput: { result: { content: [{ type: "text", text: "3 hits" }] }, error: null } });
        expect(toolOutput(mcp)?.text).toBe("3 hits");
    });

    it("keeps the picture an MCP call sent back", () => {
        const shot = call({
            title: "mcp__sikemux-tools__browser_screenshot",
            kind: "other",
            rawOutput: [{ type: "image", source: { type: "base64" } }],
            content: [{ type: "content", content: { type: "image", data: "SEVMTE8=", mimeType: "image/png" } }],
        });
        expect(toolOutput(shot)).toEqual({ text: "", cut: false, image: { data: "SEVMTE8=", mimeType: "image/png" } });
    });

    it("says an empty command printed nothing, rather than nothing at all", () => {
        expect(toolOutput(call({ rawOutput: "" }))).toEqual({ text: "", cut: false });
    });

    it("keeps the head of a long output and says it was cut", () => {
        const long = Array.from({ length: 1000 }, (_, index) => `line ${index}`).join("\n");
        const output = toolOutput(call({ rawOutput: long }));
        expect(output?.cut).toBe(true);
        expect(output?.text.split("\n")).toHaveLength(400);
        expect(output?.text.startsWith("line 0\n")).toBe(true);
    });

    it("leaves reads and edits alone, since their rows already show what they touched", () => {
        expect(toolOutput(call({ kind: "read", title: "src/app.ts", rawOutput: "export {}" }))).toBeNull();
        expect(toolOutput(call({ kind: "edit", title: "src/app.ts", rawOutput: "ok" }))).toBeNull();
    });
});

describe("toolDescription", () => {
    it("reads what Claude said a command is for", () => {
        expect(toolDescription(call({ rawInput: { command: "pnpm build", description: " Build the site " } }))).toBe("Build the site");
    });

    it("has nothing for a Codex command, or for a call that is not a command", () => {
        expect(toolDescription(call({ rawInput: { command: "pnpm build", cwd: "/repo" } }))).toBeNull();
        expect(toolDescription(call({ kind: "read", rawInput: { description: "Read it" } }))).toBeNull();
    });
});
