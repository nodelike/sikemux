import type { PromptContext } from "../api/acp";
/** Where an `@` mention of another agent points; the message's own text already shows the mention. */
export const MENTION_URI = "sikemux://agent/";

export const isMention = (uri: string): boolean => uri.startsWith(MENTION_URI);

import type { SentContext } from "./types";

export interface ContextChip {
    kind: "issue" | "pull" | null;
    number: string | null;
    title: string;
}

export function contextChip(item: SentContext): ContextChip {
    const tracked = /\/(issues|pull|pulls|pull-requests|merge_requests)\/(\d+)(?:[/?#]|$)/.exec(item.uri);
    const number = tracked?.[2] ?? null;
    const title = number ? item.title.replace(new RegExp(`^(?:(?:Issue|Pull request) )?#${number}:?\\s*`), "") : item.title;
    return { kind: tracked ? (tracked[1] === "issues" ? "issue" : "pull") : null, number, title: title || item.uri };
}

const fenceFor = (text: string) => "`".repeat(Math.max(2, ...(text.match(/`+/g) ?? []).map((run) => run.length)) + 1);

/** A context item as plain text, the way an agent that cannot take it embedded reads it. */
export function contextAsText(item: PromptContext): string {
    const fence = fenceFor(item.text);
    return `### ${item.title}\n${item.uri}\n\n${fence}\n${item.text}\n${fence}`;
}
