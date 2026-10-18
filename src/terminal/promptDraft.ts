import type { IBufferCell, Terminal } from "@xterm/xterm";

/** The rows on screen, top to bottom, with dim text such as an empty prompt's placeholder left out. */
export function screenRows(term: Pick<Terminal, "buffer" | "rows">): string[] {
    const buffer = term.buffer.active;
    const rows: string[] = [];
    let cell: IBufferCell | undefined;
    for (let row = buffer.viewportY; row < buffer.viewportY + term.rows; row++) {
        const line = buffer.getLine(row);
        if (!line) continue;
        let text = "";
        for (let x = 0; x < line.length; x++) {
            cell = line.getCell(x, cell);
            if (!cell || cell.getWidth() === 0) continue;
            text += cell.isDim() ? " " : cell.getChars() || " ";
        }
        rows.push(text.trimEnd());
    }
    return rows;
}

/** Claude's input line. The same marker points at a choice in its menus, so only a line right under a rule counts. */
const PROMPT = /^\s*❯\s?(.*)$/;
const RULE = /^\s*[─━]{8,}\s*$/;
/** How far up from the bottom the input box can sit, below the transcript. */
const BOTTOM_ROWS = 20;

/** What is typed in the input box of an agent's TUI, from its rows on screen; empty when there is none. */
export function promptDraft(rows: readonly string[]): string {
    const from = Math.max(1, rows.length - BOTTOM_ROWS);
    let start = -1;
    for (let row = rows.length - 1; row >= from; row--) {
        if (PROMPT.test(rows[row]) && RULE.test(rows[row - 1])) {
            start = row;
            break;
        }
    }
    if (start < 0) return "";
    const lines = [rows[start].replace(PROMPT, "$1")];
    for (let row = start + 1; row < rows.length && !RULE.test(rows[row]); row++) {
        if (!/^\s{2}/.test(rows[row])) return "";
        lines.push(rows[row].slice(2));
    }
    return lines
        .map((line) => line.trimEnd())
        .join("\n")
        .trim();
}
