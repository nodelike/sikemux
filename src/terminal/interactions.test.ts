import { describe, expect, it } from "vitest";
import { isTerminalFindShortcut, safeWebUrl, sanitizeTerminalTitle, terminalBufferText } from "./interactions";

describe("terminal interactions", () => {
    it("sanitizes and bounds process-controlled titles without splitting Unicode", () => {
        expect(sanitizeTerminalTitle("\u0000 build\n  ready\t")).toBe("build ready");
        expect(sanitizeTerminalTitle("\u0000\n")).toBeNull();
        expect(Array.from(sanitizeTerminalTitle("🧪".repeat(140)) ?? "")).toHaveLength(120);
        expect(sanitizeTerminalTitle("🧪".repeat(140))).toMatch(/…$/);
    });

    it("accepts only credential-free http(s) links", () => {
        expect(safeWebUrl("https://example.com/a?q=1")).toBe("https://example.com/a?q=1");
        expect(safeWebUrl("http://example.com")).toBe("http://example.com/");
        expect(safeWebUrl("file:///etc/passwd")).toBeNull();
        expect(safeWebUrl("javascript:alert(1)")).toBeNull();
        expect(safeWebUrl("https://user:secret@example.com")).toBeNull();
    });

    it("copies buffer text without adding newlines inside wrapped output", () => {
        const rows = [
            { text: "long ", isWrapped: false },
            { text: "command", isWrapped: true },
            { text: " output", isWrapped: true },
            { text: "", isWrapped: false },
            { text: "done", isWrapped: false },
            { text: "", isWrapped: false },
        ];
        const buffer = {
            length: rows.length,
            getLine: (row: number) => {
                const line = rows[row];
                return line ? { isWrapped: line.isWrapped, translateToString: () => line.text } : undefined;
            },
        };

        expect(terminalBufferText(buffer as never)).toBe("long command output\n\ndone");
    });

    it("uses the platform primary modifier for terminal find", () => {
        const key = { code: "KeyF", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false };
        expect(isTerminalFindShortcut(key, true)).toBe(true);
        expect(isTerminalFindShortcut(key, false)).toBe(false);
        expect(isTerminalFindShortcut({ ...key, metaKey: false, ctrlKey: true }, false)).toBe(true);
        expect(isTerminalFindShortcut({ ...key, shiftKey: true }, true)).toBe(false);
    });
});
