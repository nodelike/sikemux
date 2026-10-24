import { describe, expect, it } from "vitest";
import { isTerminalFindShortcut } from "./interactions";

describe("terminal interactions", () => {
    it("uses the platform primary modifier for terminal find", () => {
        const key = { code: "KeyF", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false };
        expect(isTerminalFindShortcut(key, true)).toBe(true);
        expect(isTerminalFindShortcut(key, false)).toBe(false);
        expect(isTerminalFindShortcut({ ...key, metaKey: false, ctrlKey: true }, false)).toBe(true);
        expect(isTerminalFindShortcut({ ...key, shiftKey: true }, true)).toBe(false);
    });
});
