import { toolKind, toolTarget } from "./toolLabels";
import type { AcpAsyncTask, AcpSubagent, ChatMessage, ChatPart } from "./types";

const MAX_DETAIL_CHARS = 120_000;

export function formatDetail(value: unknown): string {
    let formatted: string;
    try {
        formatted = JSON.stringify(value, null, 2) ?? String(value);
    } catch {
        formatted = String(value);
    }
    return formatted.length > MAX_DETAIL_CHARS ? `${formatted.slice(0, MAX_DETAIL_CHARS)}\n… output truncated` : formatted;
}

/* An attachment arrives as bytes and a type, with nothing naming it, so the
   type is the only thing that can say what it would be saved as. */
export function attachmentName(mimeType: string): string {
    const kind =
        mimeType
            .split("/")
            .pop()
            ?.split("+")[0]
            ?.replace(/[^a-z0-9]/gi, "") || "png";
    return `attachment.${kind}`;
}

export function decodedFenceName(info: string): string {
    try {
        return decodeURIComponent(info);
    } catch {
        return info;
    }
}

export type PartGroup = { id: string; tools: Extract<ChatPart, { kind: "tool" }>[] } | { id: string; part: ChatPart };

/* Consecutive calls share one group, except a page an agent showed, which is
   part of its reply rather than a step towards it. */
export function groupParts(parts: ChatPart[]): PartGroup[] {
    const groups: PartGroup[] = [];
    for (const part of parts) {
        const last = groups.at(-1);
        if (part.kind !== "tool" || part.page) groups.push({ id: part.id, part });
        else if (last && "tools" in last) last.tools.push(part);
        else groups.push({ id: part.id, tools: [part] });
    }
    return groups;
}

/* A finished turn is a run of assistant messages: the work, then the reply it
   closed with. `from` is where that reply starts in the run's last message. */
export interface WorkFold {
    /** The id of the turn's first message, which names the fold for as long as the turn is shown. */
    id: string;
    start: number;
    end: number;
    from: number;
    calls: number;
    took: number | null;
}

const isReply = (part: ChatPart) => part.kind === "text" || part.kind === "content";

/* Runs while a reply streams in, so it walks the turn's parts in place rather than gathering them. */
function workFold(messages: ChatMessage[], start: number, end: number): WorkFold | null {
    const parts = messages[end].parts;
    let from = parts.length;
    let said = false;
    while (from > 0 && isReply(parts[from - 1])) {
        from -= 1;
        said ||= parts[from].kind === "text";
    }
    if (!said) return null;
    let calls = 0;
    let worked = false;
    for (let index = start; index <= end; index += 1) {
        const work = messages[index].parts;
        const until = index === end ? from : work.length;
        for (let at = 0; at < until; at += 1) {
            if (work[at].kind === "tool") calls += 1;
            worked ||= !isReply(work[at]);
        }
    }
    if (!worked) return null;
    return { id: messages[start].id, start, end, from, calls, took: workTime(messages, start, end) };
}

/* A prompted turn starts with its prompt; one the agent began by itself starts
   when the core said it did. */
function workTime(messages: ChatMessage[], start: number, end: number): number | null {
    const before = messages[start - 1];
    const began = (before?.role === "user" ? before.sentAt : undefined) ?? messages[start].opensTurn?.at ?? null;
    const ended = messages[end].endedAt ?? null;
    return began !== null && ended !== null && ended >= began ? ended - began : null;
}

const sameFold = (a: WorkFold | undefined, b: WorkFold) =>
    a !== undefined && a.id === b.id && a.start === b.start && a.end === b.end && a.from === b.from && a.calls === b.calls && a.took === b.took;

/**
 * Every finished turn worth folding, by the index of each message in it. A list redraws its rows when this
 * changes, so `previous` comes back as it is when nothing in it did.
 */
export function workFolds(messages: ChatMessage[], running: boolean, previous?: Map<number, WorkFold>): Map<number, WorkFold> {
    const folds = new Map<number, WorkFold>();
    for (let start = 0; start < messages.length; start += 1) {
        if (messages[start].role !== "assistant") continue;
        let end = start;
        while (messages[end + 1]?.role === "assistant" && !messages[end + 1].opensTurn) end += 1;
        const fold = running && end === messages.length - 1 ? null : workFold(messages, start, end);
        if (fold) for (let index = start; index <= end; index += 1) folds.set(index, fold);
        start = end;
    }
    if (previous && previous.size === folds.size && [...folds].every(([index, fold]) => sameFold(previous.get(index), fold))) return previous;
    return folds;
}

/* A subagent is handed a whole prompt as its task, and a prompt is paragraphs.
   The row is one line, so it opens with the first line and the tooltip keeps
   the rest. */
export function subagentTask(task: string): string {
    return task.split("\n")[0].trim();
}

/* What a subagent is up to, taken from the last thing it sent. A tool it is
   part-way through says more than the prose it wrote before starting. */
export function subagentActivity(subagent: AcpSubagent): string {
    for (let index = subagent.messages.length - 1; index >= 0; index -= 1) {
        const parts = subagent.messages[index].parts;
        for (let position = parts.length - 1; position >= 0; position -= 1) {
            const part = parts[position];
            if (part.kind === "tool") return `${toolKind(part.tool)} ${toolTarget(part.tool)}`.trim();
        }
    }
    return subagentTask(subagent.task);
}

export function runningSubagents(messages: ChatMessage[]): AcpSubagent[] {
    const running: AcpSubagent[] = [];
    for (const message of messages)
        for (const part of message.parts) if (part.kind === "subagent" && part.subagent.state === "running") running.push(part.subagent);
    return running;
}

/* A task whose description repeats its name would print the same words twice,
   once in each voice, so the detail takes the first thing that says more. */
export function taskDetail(task: AcpAsyncTask): string | undefined {
    return [task.summary, task.description, task.lastToolName, task.taskType].find((text) => text && text !== task.name);
}

/* An agent names its own task types — "shell", "monitor" — and they are the
   only thing that separates one background task from another, so they are what
   the groups are cut on. */
export function groupTasks(tasks: AcpAsyncTask[]): [string, AcpAsyncTask[]][] {
    const groups = new Map<string, AcpAsyncTask[]>();
    for (const task of tasks) {
        const kind = task.taskType || "task";
        const existing = groups.get(kind);
        if (existing) existing.push(task);
        else groups.set(kind, [task]);
    }
    return [...groups];
}
