import type { ChatMessage } from "./types";

export interface ChatFindOptions {
    caseSensitive: boolean;
    wholeWord: boolean;
}

/** One place the query was found: the message it is in, and which occurrence within that message. */
export interface ChatFindMatch {
    message: number;
    occurrence: number;
}

/** The query as a pattern, or null while there is nothing to look for. */
export function findPattern(query: string, { caseSensitive, wholeWord }: ChatFindOptions): RegExp | null {
    if (!query) return null;
    const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(wholeWord ? `\\b${escaped}\\b` : escaped, caseSensitive ? "g" : "gi");
}

/** What a reader can read of a message: its prose, not the tool calls it made. */
export function messageSearchText(message: ChatMessage): string {
    return message.parts.map((part) => (part.kind === "text" ? part.text : "")).join("\n");
}

export function findMatches(messages: readonly ChatMessage[], pattern: RegExp | null): ChatFindMatch[] {
    if (!pattern) return [];
    const matches: ChatFindMatch[] = [];
    messages.forEach((message, index) => {
        const count = messageSearchText(message).match(pattern)?.length ?? 0;
        for (let occurrence = 0; occurrence < count; occurrence += 1) matches.push({ message: index, occurrence });
    });
    return matches;
}

/**
 * Every place `pattern` matches the text shown inside `root`, as ranges. The
 * text is read as one run across elements, so a match that crosses bold, a
 * link or inline code is still one range.
 */
export function rangesIn(root: Node, pattern: RegExp): Range[] {
    const nodes: Text[] = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) nodes.push(node as Text);
    const starts: number[] = [];
    let text = "";
    for (const node of nodes) {
        starts.push(text.length);
        text += node.data;
    }
    // The text node holding the character at `offset`, and where in it that character is.
    const locate = (offset: number): [Text, number] => {
        let index = starts.length - 1;
        while (index > 0 && starts[index] > offset) index -= 1;
        return [nodes[index], offset - starts[index]];
    };
    const ranges: Range[] = [];
    for (const match of text.matchAll(new RegExp(pattern.source, pattern.flags))) {
        if (match[0].length === 0) continue;
        const range = document.createRange();
        range.setStart(...locate(match.index));
        range.setEnd(...locate(match.index + match[0].length - 1));
        range.setEnd(range.endContainer, range.endOffset + 1);
        ranges.push(range);
    }
    return ranges;
}
