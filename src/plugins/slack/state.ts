import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import type { SlackThread } from "./api";
import { SLACK_MESSAGES } from "./kinds";

export interface SlackView {
    /** The workspace shown; empty for the default one. */
    workspace: string;
    channel: string | null;
    /** The thread open beside the channel. */
    thread: { channel: string; ts: string } | null;
}

const views = definePaneViews<SlackView>({ workspace: "", channel: null, thread: null });

export const useSlackView = views.use;
export const updateSlackView = views.update;

export const openSlack = (): string | null => openSurface(SLACK_MESSAGES);

/** A thread as an agent reads it: where it is, then each message with who said it and when. */
export function threadForAgent(thread: SlackThread, channelName: string | null): string {
    const where = channelName ? `#${channelName}` : thread.channel;
    const lines = thread.messages.map((message) => {
        const files = message.files.length > 0 ? `\n  (attached: ${message.files.join(", ")})` : "";
        return `**${message.name}** (${message.at}):\n${message.text}${files}`;
    });
    const header = `Slack thread in ${where}${thread.permalink ? ` — ${thread.permalink}` : ""}`;
    const cut = thread.truncated ? "\n\n(The thread is longer; only the first 1000 messages are here.)" : "";
    const reply = thread.permalink ? `\n\nTo reply in this thread, use slack_post with link ${thread.permalink}.` : "";
    return `${header}\n\n${lines.join("\n\n")}${cut}${reply}`;
}
