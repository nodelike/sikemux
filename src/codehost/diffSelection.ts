import { codeFence } from "../agents/agentTargets";
import type { AgentDelivery } from "../agents/agentInbox";
import { DIFF_ROW, type DiffRow } from "../api/git";

function rowIndexOf(node: Node | null, within: HTMLElement): number | null {
    const element = node instanceof Element ? node : node?.parentElement;
    const row = element?.closest<HTMLElement>("[data-row]");
    if (!row || !within.contains(row)) return null;
    return Number(row.dataset.row);
}

/** The rows the text selection covers inside this diff, or else the row that was clicked. */
export function pickedRows(within: HTMLElement, selection: Selection | null, clicked: EventTarget | null): [number, number] | null {
    if (selection && !selection.isCollapsed) {
        const from = rowIndexOf(selection.anchorNode, within);
        const to = rowIndexOf(selection.focusNode, within);
        if (from !== null && to !== null) return from <= to ? [from, to] : [to, from];
    }
    const row = rowIndexOf(clicked instanceof Node ? clicked : null, within);
    return row === null ? null : [row, row];
}

function span(numbers: number[]): string {
    const first = Math.min(...numbers);
    const last = Math.max(...numbers);
    return first === last ? `line ${first}` : `lines ${first}–${last}`;
}

export interface PullRef {
    number: number;
    title: string;
    url: string;
}

export function diffLinesDelivery(pull: PullRef, path: string, rows: readonly DiffRow[]): AgentDelivery {
    const lines = rows.filter((row) => row[0] !== DIFF_ROW.hidden);
    const oldNumbers = lines.filter((row) => row[0] === DIFF_ROW.deleted).map((row) => row[1]);
    const newNumbers = lines.filter((row) => row[0] !== DIFF_ROW.deleted).map((row) => row[1]);
    const where = [oldNumbers.length ? `old ${span(oldNumbers)}` : null, newNumbers.length ? `new ${span(newNumbers)}` : null]
        .filter(Boolean)
        .join(" and ");
    const marks = { [DIFF_ROW.context]: " ", [DIFF_ROW.added]: "+", [DIFF_ROW.deleted]: "-" } as Record<number, string>;
    const diff = lines.map((row) => `${marks[row[0]]}${row[2]}`).join("\n");
    return {
        text: `From pull request #${pull.number} "${pull.title}" (${pull.url}), ${path}, ${where}:\n\n${codeFence(diff, "diff")}\n`,
    };
}
