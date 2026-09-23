import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Terminal } from "@xterm/xterm";
import {
    applyTerminalFontSize,
    clampTerminalFontSize,
    currentTerminalFontSize,
    DEFAULT_TERMINAL_FONT_SIZE,
    registerTerminalFontSize,
} from "./fontSize";

function fakeTerminal() {
    return { options: {} } as unknown as Terminal;
}

describe("terminal font size", () => {
    beforeEach(() => {
        applyTerminalFontSize(DEFAULT_TERMINAL_FONT_SIZE);
    });

    it("keeps sizes inside the readable range and rounds fractions", () => {
        expect(clampTerminalFontSize(4)).toBe(8);
        expect(clampTerminalFontSize(999)).toBe(32);
        expect(clampTerminalFontSize(13.4)).toBe(13);
        expect(clampTerminalFontSize(Number.NaN)).toBe(DEFAULT_TERMINAL_FONT_SIZE);
    });

    it("gives a terminal the current size as it registers", () => {
        applyTerminalFontSize(18);
        const term = fakeTerminal();
        registerTerminalFontSize({ term, refit: () => {} });
        expect(term.options.fontSize).toBe(18);
    });

    it("resizes every live terminal and refits it so the pty learns the new grid", () => {
        const first = fakeTerminal();
        const second = fakeTerminal();
        const refitFirst = vi.fn();
        const refitSecond = vi.fn();
        registerTerminalFontSize({ term: first, refit: refitFirst });
        registerTerminalFontSize({ term: second, refit: refitSecond });

        applyTerminalFontSize(20);

        expect(currentTerminalFontSize()).toBe(20);
        expect(first.options.fontSize).toBe(20);
        expect(second.options.fontSize).toBe(20);
        expect(refitFirst).toHaveBeenCalledTimes(1);
        expect(refitSecond).toHaveBeenCalledTimes(1);
    });

    it("leaves a disposed terminal alone", () => {
        const term = fakeTerminal();
        const refit = vi.fn();
        const dispose = registerTerminalFontSize({ term, refit });
        dispose();

        applyTerminalFontSize(22);

        expect(term.options.fontSize).toBe(DEFAULT_TERMINAL_FONT_SIZE);
        expect(refit).not.toHaveBeenCalled();
    });
});
