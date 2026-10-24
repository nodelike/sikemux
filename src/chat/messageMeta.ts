import type { ChatMessage } from "./types";

export function rateLabel(rate: number): string {
    return `~${rate < 10 ? rate.toFixed(1) : Math.round(rate)} tok/s`;
}

/* What a reader means by "copy this message": the prose, the agent's reasoning
   where it showed it, and the files a turn was sent with. Tool rows are the
   pane's own bookkeeping and stay behind. */
export function messageText(message: ChatMessage): string {
    const blocks = message.parts
        .map((part) => (part.kind === "text" || part.kind === "thought" ? part.text.trim() : ""))
        .filter((text) => text.length > 0);
    for (const path of message.attachments ?? []) blocks.push(path);
    return blocks.join("\n\n");
}

/** What copying a row gives: a prompt's own text, or at the end of an answer, all of the answer's prose. */
export function copyText(messages: readonly ChatMessage[], index: number): string {
    const message = messages[index];
    if (message.role === "user") return messageText(message);
    if (!message.meta) return "";
    const from = messages.findIndex((candidate) => candidate.id === message.meta?.from);
    return messages
        .slice(from < 0 ? index : from, index + 1)
        .map(messageText)
        .filter(Boolean)
        .join("\n\n");
}

const DAY_MS = 24 * 60 * 60 * 1000;
const startOfDay = (at: number): number => new Date(at).setHours(0, 0, 0, 0);

/** When a message was sent, as a reader scanning the transcript wants it: "Today, 21:07", "Yesterday, 14:02", "29 Sep, 09:15". */
export function sentLabel(at: number, now = Date.now(), locale?: string): string {
    const date = new Date(at);
    const time = date.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
    const days = Math.round((startOfDay(now) - startOfDay(at)) / DAY_MS);
    if (days === 0) return `Today, ${time}`;
    if (days === 1) return `Yesterday, ${time}`;
    const sameYear = date.getFullYear() === new Date(now).getFullYear();
    const day = date.toLocaleDateString(locale, { day: "numeric", month: "short", ...(sameYear ? {} : { year: "numeric" }) });
    return `${day}, ${time}`;
}

/** The whole moment, for a tooltip: "Tuesday, 29 September 2026 at 21:07:15". */
export function sentTitle(at: number, locale?: string): string {
    return new Date(at).toLocaleString(locale, { dateStyle: "full", timeStyle: "medium" });
}
