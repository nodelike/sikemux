import type { PromptContext } from "../api/acp";

/** What a sent message keeps of a context item: enough to name it, not its whole text. */
export type SentContext = Pick<PromptContext, "uri" | "title">;

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

/** A sent message's text with its context items read back out of it, for a session loaded from disk. */
export function splitSentContext(text: string): { text: string; context: SentContext[] } {
    const context: SentContext[] = [];
    let rest = text.replace(/\n?<context ref="([^"]+)">\n([\s\S]*?)\n<\/context>/g, (_whole, uri: string, body: string) => {
        context.push({ uri, title: body.split("\n", 1)[0] });
        return "";
    });
    for (const { uri } of context) rest = rest.replace(uri, "");
    rest = rest.replace(/(?:^|\n\n)### (.+)\n(\S+)\n\n(`{3,})\n[\s\S]*?\n\3(?=\n\n###|$)/g, (_whole, title: string, uri: string) => {
        context.push({ uri, title });
        return "";
    });
    return { text: context.length ? rest.trim() : text, context };
}
