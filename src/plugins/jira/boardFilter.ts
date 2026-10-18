import type { JiraBoardView, JiraColumn, JiraPerson } from "./api";

export interface BoardPerson {
    accountId: string;
    name: string;
    avatarUrl: string | null;
    count: number;
}

/** Everyone with a card on the board, busiest first. */
export function boardPeople(board: JiraBoardView): BoardPerson[] {
    const people = new Map<string, BoardPerson>();
    for (const issue of board.columns.flatMap((column) => column.issues)) {
        if (!issue.assignee) continue;
        const seen = people.get(issue.assignee.accountId);
        if (seen) seen.count += 1;
        else people.set(issue.assignee.accountId, { ...issue.assignee, count: 1 });
    }
    return [...people.values()].sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

export const unassignedCount = (board: JiraBoardView) =>
    board.columns.reduce((total, column) => total + column.issues.filter((issue) => !issue.assignee).length, 0);

/** The same picks always make the same key, so a board narrowed to them is fetched once. */
export const pickKey = (picked: ReadonlySet<string>) => [...picked].sort().join(",");

/** The cards whose key or title holds the typed words. */
export function matchingCards(columns: JiraColumn[], words: string): JiraColumn[] {
    const needle = words.trim().toLowerCase();
    if (!needle) return columns;
    return columns.map((column) => ({
        ...column,
        issues: column.issues.filter((issue) => issue.key.toLowerCase().includes(needle) || issue.summary.toLowerCase().includes(needle)),
    }));
}

export interface Swimlane {
    id: string;
    person: JiraPerson | null;
    count: number;
    columns: JiraColumn[];
}

/** The board split into one row of columns per assignee, as Jira's "Group: Assignee" does: busiest first, nobody last. */
export function swimlanes(columns: JiraColumn[]): Swimlane[] {
    const lanes = new Map<string, Swimlane>();
    columns.forEach((column, index) => {
        for (const issue of column.issues) {
            const id = issue.assignee?.accountId ?? "";
            let lane = lanes.get(id);
            if (!lane) {
                lane = { id, person: issue.assignee, count: 0, columns: columns.map((each) => ({ ...each, issues: [] })) };
                lanes.set(id, lane);
            }
            lane.count += 1;
            lane.columns[index]?.issues.push(issue);
        }
    });
    return [...lanes.values()].sort(
        (a, b) => Number(!a.person) - Number(!b.person) || b.count - a.count || (a.person?.name ?? "").localeCompare(b.person?.name ?? ""),
    );
}
