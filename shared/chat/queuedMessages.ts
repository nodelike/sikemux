import type { PromptContext } from "./promptContext";
import { basename } from "../lib/paths";
import type { QueuedMessage } from "./types";

/** What the composer hands over when the person sends. */
export interface OutgoingMessage {
    text: string;
    paths: string[];
    context: PromptContext[];
}

export const queuedLabel = (message: QueuedMessage): string =>
    message.text || [...message.paths.map(basename), ...message.context.map((item) => item.title)].join(", ");
