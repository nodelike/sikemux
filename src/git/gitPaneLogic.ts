export const isInRange = (range: [number, number] | null, i: number): boolean => !!range && i >= range[0] && i <= range[1];

export const helpRows = (...rows: [keys: string, label: string][]): { keys: string; label: string }[] =>
    rows.map(([keys, label]) => ({ keys, label }));

export function filterByQuery<T>(items: T[], query: string, fields: (item: T) => (string | null | undefined)[]): T[] {
    if (!query) return items;
    const q = query.toLowerCase();
    return items.filter((item) => fields(item).some((v) => (v ?? "").toLowerCase().includes(q)));
}
