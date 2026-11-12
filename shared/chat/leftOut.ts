import type { ChatMessage } from "../wire.generated";

/** How many sentences that rested on no source were left out of an answer. */
export function leftOutCount(message: ChatMessage): number {
    return (message.leftOut ?? []).reduce((sum, out) => sum + out.sentences, 0);
}

/** The quiet line under such an answer, as the Mac words it. */
export const leftOutLine = (count: number) => (count === 1 ? "1 sentence with no source left out" : `${count} sentences with no source left out`);

export const SHOW = "Show";
export const HIDE = "Hide";
