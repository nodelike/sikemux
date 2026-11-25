import type { SentContext } from "../wire.generated";

const ARTIFACT_URI = "sikemux://artifact/";

export interface ContextChip {
    kind: "issue" | "pull" | "artifact" | null;
    number: string | null;
    title: string;
}

export function contextChip(item: SentContext): ContextChip {
    return {
        kind: item.tracked?.kind ?? (item.uri.startsWith(ARTIFACT_URI) ? "artifact" : null),
        number: item.tracked?.number ?? null,
        title: item.tracked?.title ?? item.title,
    };
}
