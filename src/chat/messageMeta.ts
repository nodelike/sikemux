import type { ChatMessage } from "./types";

/* Nothing in the session reports how many tokens the agent wrote, so the rate
   is read off the characters that arrived: four per token is the ratio English
   prose and code both land near. The number is shown with a "~" so it reads as
   the estimate it is. */
const CHARS_PER_TOKEN = 4;

/* A burst shorter than this is one network hiccup away from any answer at all,
   so it gets no number rather than a wrong one. */
const MIN_SAMPLE_MS = 400;

export interface RowMeta {
    text: string;
    rate: number | null;
    /** When a prompt was sent, or when an answer's turn finished. */
    at: number | null;
    /** How long the turn took, from the prompt that started it to its finish. */
    took: number | null;
}

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

/* The agent opens a fresh message every time it comes back from a tool, so one
   answer is a run of them. The run is read as the one thing it is: all of its
   prose, and the time spent writing rather than the time spent waiting on the
   tools in between. */
function answerMeta(messages: ChatMessage[], endIndex: number): RowMeta {
    const blocks: string[] = [];
    let chars = 0;
    let written = 0;
    let started: number | null = null;
    let index = endIndex;
    for (; index >= 0 && messages[index].role === "assistant"; index -= 1) {
        const message = messages[index];
        started = message.sentAt ?? started;
        const text = messageText(message);
        if (text) blocks.unshift(text);
        const span =
            message.streamStartedAt !== undefined && message.streamEndedAt !== undefined ? message.streamEndedAt - message.streamStartedAt : 0;
        if (span <= 0) continue;
        chars += message.streamChars ?? 0;
        written += span;
    }
    const rate = chars > 0 && written >= MIN_SAMPLE_MS ? chars / CHARS_PER_TOKEN / (written / 1000) : null;
    const at = messages[endIndex].endedAt ?? null;
    started = messages[index]?.sentAt ?? started;
    return { text: blocks.join("\n\n"), rate, at, took: at !== null && started !== null ? at - started : null };
}

/* A prompt carries its own copy. An answer carries one only where it ends, so
   a turn gets a single row rather than one after every tool call. */
export function rowMeta(messages: ChatMessage[], index: number): RowMeta {
    const message = messages[index];
    if (message.role === "user") return { text: messageText(message), rate: null, at: message.sentAt ?? null, took: null };
    if (messages[index + 1]?.role === "assistant") return { text: "", rate: null, at: null, took: null };
    return answerMeta(messages, index);
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
