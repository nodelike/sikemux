import { DIFF_ROW, type DiffRow } from "../api/git";

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/u;

/**
 * A host's unified patch for one file as the rows the git pane's diff draws: added and unchanged lines numbered in the
 * new file, removed ones in the old, and the unchanged lines between hunks as a count, since the patch leaves them out.
 */
export function patchToRows(patch: string): DiffRow[] {
    const lines = patch.split("\n");
    if (lines.at(-1) === "") lines.pop();
    const rows: DiffRow[] = [];
    let oldLine = 0;
    let newLine = 0;
    let shownUpTo = 0;
    for (const line of lines) {
        const hunk = HUNK.exec(line);
        if (hunk) {
            oldLine = Number(hunk[1]);
            newLine = Number(hunk[2]);
            const skipped = newLine - 1 - shownUpTo;
            if (skipped > 0) rows.push([DIFF_ROW.hidden, skipped, ""]);
            continue;
        }
        if (newLine === 0 && oldLine === 0) continue;
        const text = line.slice(1);
        if (line.startsWith("+")) {
            rows.push([DIFF_ROW.added, newLine, text]);
            shownUpTo = newLine;
            newLine += 1;
        } else if (line.startsWith("-")) {
            rows.push([DIFF_ROW.deleted, oldLine, text]);
            oldLine += 1;
        } else if (!line.startsWith("\\")) {
            rows.push([DIFF_ROW.context, newLine, text]);
            shownUpTo = newLine;
            newLine += 1;
            oldLine += 1;
        }
    }
    return rows;
}
