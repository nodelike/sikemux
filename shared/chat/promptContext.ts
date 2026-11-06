import type { SentContext } from "./types";

export interface ContextChip {
    kind: "issue" | "pull" | null;
    number: string | null;
    title: string;
}

export function contextChip(item: SentContext): ContextChip {
    return {
        kind: item.tracked?.kind ?? null,
        number: item.tracked?.number ?? null,
        title: item.tracked?.title ?? item.title,
    };
}
