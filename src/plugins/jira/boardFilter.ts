import type { JiraColumn } from "./api";

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
