import type { TokensResult } from "shiki/core";
import type { CodeLine, CodeToken } from "@shared/chat/types";

const FONT_ITALIC = 1;
const FONT_BOLD = 2;
const FONT_UNDERLINE = 4;

function sameStyle(a: CodeToken, b: CodeToken): boolean {
    return a.color === b.color && a.italic === b.italic && a.bold === b.bold && a.underline === b.underline;
}

export function codeLines(highlighted: TokensResult): CodeLine[] {
    const plain = highlighted.fg?.toLowerCase();
    return highlighted.tokens.map((line) => {
        const merged: CodeToken[] = [];
        for (const token of line) {
            const style = token.fontStyle ?? 0;
            const color = token.color && token.color.toLowerCase() !== plain ? token.color : undefined;
            const next: CodeToken = {
                text: token.content,
                ...(color ? { color } : {}),
                ...(style & FONT_ITALIC ? { italic: true } : {}),
                ...(style & FONT_BOLD ? { bold: true } : {}),
                ...(style & FONT_UNDERLINE ? { underline: true } : {}),
            };
            // A line is mostly one colour in runs, and every run that stays one
            // span is one element fewer in a list that is already long.
            const last = merged[merged.length - 1];
            if (last && sameStyle(last, next)) merged[merged.length - 1] = { ...last, text: last.text + next.text };
            else merged.push(next);
        }
        return merged;
    });
}
