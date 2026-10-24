import { beforeEach, describe, expect, it, vi } from "vitest";
import {
    applyTerminalFontSize,
    clampTerminalFontSize,
    currentTerminalFontSize,
    DEFAULT_TERMINAL_FONT_SIZE,
    registerTerminalFontSize,
} from "./fontSize";

function fakeTerminal() {
    const terminal = { fontSize: 0, setFontSize: (size: number) => (terminal.fontSize = size) };
    return terminal;
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

    it("resizes every live terminal and refits it so the pty learns the new grid", () => {
        const first = fakeTerminal();
        const second = fakeTerminal();
        const refitFirst = vi.fn();
        const refitSecond = vi.fn();
        registerTerminalFontSize({ setFontSize: first.setFontSize, refit: refitFirst });
        registerTerminalFontSize({ setFontSize: second.setFontSize, refit: refitSecond });

        applyTerminalFontSize(20);

        expect(currentTerminalFontSize()).toBe(20);
        expect(first.fontSize).toBe(20);
        expect(second.fontSize).toBe(20);
        expect(refitFirst).toHaveBeenCalledTimes(1);
        expect(refitSecond).toHaveBeenCalledTimes(1);
    });

    it("leaves a disposed terminal alone", () => {
        const term = fakeTerminal();
        const refit = vi.fn();
        const dispose = registerTerminalFontSize({ setFontSize: term.setFontSize, refit });
        dispose();

        applyTerminalFontSize(22);

        expect(term.fontSize).toBe(DEFAULT_TERMINAL_FONT_SIZE);
        expect(refit).not.toHaveBeenCalled();
    });
});
