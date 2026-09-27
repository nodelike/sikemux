import type { GitAiProvider, RightView } from "./gitPaneTypes";

export const isGitAiProvider = (value: string | null): value is GitAiProvider => value === "hermes" || value === "codex" || value === "claude";

export const rangeBadge = (range: [number, number] | null): string | null => (range ? `range ${range[1] - range[0] + 1}` : null);

export const isInRange = (range: [number, number] | null, i: number): boolean => !!range && i >= range[0] && i <= range[1];

export const helpRows = (...rows: [keys: string, label: string][]): { keys: string; label: string }[] =>
    rows.map(([keys, label]) => ({ keys, label }));

export function filterByQuery<T>(items: T[], query: string, fields: (item: T) => (string | null | undefined)[]): T[] {
    if (!query) return items;
    const q = query.toLowerCase();
    return items.filter((item) => fields(item).some((v) => (v ?? "").toLowerCase().includes(q)));
}

export function sameRightView(a: RightView, b: RightView): boolean {
    if (a.mode === "merge" && b.mode === "merge") return a.files === b.files;
    if (a.mode === "commit" && b.mode === "commit") return a.rev === b.rev && a.title === b.title && a.subtitle === b.subtitle;
    if (a.mode === "output" && b.mode === "output") return a.text === b.text;
    return false;
}
