import type { Terminal } from "@xterm/xterm";

export const DEFAULT_TERMINAL_FONT_SIZE = 13;
const MIN_TERMINAL_FONT_SIZE = 8;
const MAX_TERMINAL_FONT_SIZE = 32;

interface RegisteredTerminal {
    term: Terminal;
    refit: () => void;
}

let current = DEFAULT_TERMINAL_FONT_SIZE;
const terms = new Set<RegisteredTerminal>();

export function clampTerminalFontSize(value: number): number {
    if (!Number.isFinite(value)) return DEFAULT_TERMINAL_FONT_SIZE;
    return Math.min(MAX_TERMINAL_FONT_SIZE, Math.max(MIN_TERMINAL_FONT_SIZE, Math.round(value)));
}

export function currentTerminalFontSize(): number {
    return current;
}

export function registerTerminalFontSize(entry: RegisteredTerminal): () => void {
    entry.term.options.fontSize = current;
    terms.add(entry);
    return () => terms.delete(entry);
}

// A new cell size needs a refit, or the shell keeps drawing to the old grid.
export function applyTerminalFontSize(value: number): void {
    current = clampTerminalFontSize(value);
    terms.forEach(({ term, refit }) => {
        term.options.fontSize = current;
        refit();
    });
}
