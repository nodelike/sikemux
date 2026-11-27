import type { SentContext } from "../wire.generated";

const ARTIFACT_URI = "sikemux://artifact/";
const CALL_URI = "sikemux://call/";
const SKILL_URI = "sikemux://skill/";

export interface ContextChip {
    kind: "issue" | "pull" | "artifact" | "call" | "skill" | null;
    number: string | null;
    title: string;
}

export function contextChip(item: SentContext): ContextChip {
    if (item.uri.startsWith(SKILL_URI)) return { kind: "skill", number: null, title: item.uri.slice(SKILL_URI.length) };
    return {
        kind: item.tracked?.kind ?? (item.uri.startsWith(ARTIFACT_URI) ? "artifact" : item.uri.startsWith(CALL_URI) ? "call" : null),
        number: item.tracked?.number ?? null,
        title: item.tracked?.title ?? item.title,
    };
}
