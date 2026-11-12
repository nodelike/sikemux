import type { ChatMessage } from "../wire.generated";
import { english, type Say } from "../lib/say";

/** How many sentences that rested on no source were left out of an answer. */
export function leftOutCount(message: ChatMessage): number {
    return (message.leftOut ?? []).reduce((sum, out) => sum + out.sentences, 0);
}

/** The quiet line under such an answer, as the Mac words it. */
export const leftOutLine = (count: number, say: Say = english) =>
    count === 1 ? say("1 sentence with no source left out") : say("{count} sentences with no source left out", { count });

export const SHOW = "Show";
export const HIDE = "Hide";
