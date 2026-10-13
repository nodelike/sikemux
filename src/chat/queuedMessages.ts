import type { PromptContext } from "../api/acp";
import { basename } from "../lib/paths";

/** What the composer hands over when the person sends. */
export interface OutgoingMessage {
    text: string;
    paths: string[];
    context: PromptContext[];
}

export type QueuedMessage = OutgoingMessage & { id: string };

export const queuedLabel = (message: QueuedMessage): string =>
    message.text || [...message.paths.map(basename), ...message.context.map((item) => item.title)].join(", ");

const isCommand = (message: { text: string }) => message.text.startsWith("/");

/* A slash command only works as the whole prompt, so it goes out on its own. */
export function nextBatch<T extends { text: string }>(queued: readonly T[]): T[] {
    if (queued.length === 0 || isCommand(queued[0])) return queued.slice(0, 1);
    const command = queued.findIndex(isCommand);
    return command === -1 ? [...queued] : queued.slice(0, command);
}

function uniqueBy<T>(items: readonly T[], key: (item: T) => string): T[] {
    const seen = new Set<string>();
    return items.filter((item) => !seen.has(key(item)) && Boolean(seen.add(key(item))));
}

const joinedText = (messages: readonly { text: string }[]) =>
    messages
        .map((message) => message.text)
        .filter(Boolean)
        .join("\n\n");

/** Messages sent as one: their texts in order, and each attachment once. */
export function combine<A>(
    messages: readonly { text: string; attachments: readonly A[] }[],
    key: (attachment: A) => string,
): { text: string; attachments: A[] } {
    return {
        text: joinedText(messages),
        attachments: uniqueBy(
            messages.flatMap((message) => message.attachments),
            key,
        ),
    };
}

export function combineQueued(messages: QueuedMessage[]): QueuedMessage {
    return {
        id: messages[0].id,
        text: joinedText(messages),
        paths: uniqueBy(
            messages.flatMap((message) => message.paths),
            (path) => path,
        ),
        context: uniqueBy(
            messages.flatMap((message) => message.context),
            (item) => item.uri,
        ),
    };
}
