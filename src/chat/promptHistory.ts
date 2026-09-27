import type { ChatMessage } from "./types";

/** What was sent in this chat, oldest first, with a message repeated back to back kept once. */
export function sentPrompts(messages: readonly ChatMessage[], queued: readonly string[] = []): string[] {
    const prompts: string[] = [];
    const texts = messages
        .filter((message) => message.role === "user")
        .map((message) =>
            message.parts
                .map((part) => (part.kind === "text" ? part.text : ""))
                .join("")
                .trim(),
        );
    for (const text of [...texts, ...queued.map((text) => text.trim())]) {
        if (text && prompts.at(-1) !== text) prompts.push(text);
    }
    return prompts;
}

/** Which sent message the composer is showing, and what was typed before browsing began. */
export interface HistoryPosition {
    index: number;
    typed: string;
}

export interface Recalled {
    draft: string;
    position: HistoryPosition | null;
}

/**
 * One step through the sent messages, as a shell's up and down arrows do.
 * Stepping newer past the latest one gives back what was being typed.
 * Null when there is nowhere further to go.
 */
export function recallPrompt(
    prompts: readonly string[],
    position: HistoryPosition | null,
    draft: string,
    direction: "older" | "newer",
): Recalled | null {
    if (direction === "older") {
        const index = (position?.index ?? prompts.length) - 1;
        if (index < 0) return null;
        return { draft: prompts[index], position: { index, typed: position?.typed ?? draft } };
    }
    if (!position) return null;
    const index = position.index + 1;
    if (index >= prompts.length) return { draft: position.typed, position: null };
    return { draft: prompts[index], position: { ...position, index } };
}

/** Up and down move between lines first; only at the top or bottom edge do they move between messages. */
export function caretAtEdge(text: string, start: number, end: number, direction: "older" | "newer"): boolean {
    if (start !== end) return false;
    return direction === "older" ? !text.slice(0, start).includes("\n") : !text.slice(end).includes("\n");
}
