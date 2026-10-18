import { tokenCount } from "./contextUsage";
import { isMention, splitSentContext, type SentContext } from "./promptContext";
import { toolKind, toolTarget } from "./toolLabels";
import { callCount } from "./toolView";
import type { AcpAsyncTask, AcpSubagent, AcpWorkflowAgent, ChatMessage, ChatPart } from "./types";

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

function embeddedContext(content: Extract<ChatPart, { kind: "content" }>["content"]): SentContext | null {
    if (content.type !== "resource" || !content.resource || typeof content.resource !== "object") return null;
    const resource = content.resource as { uri?: unknown; text?: unknown };
    if (typeof resource.uri !== "string") return null;
    const firstLine = typeof resource.text === "string" ? resource.text.split("\n", 1)[0] : "";
    return { uri: resource.uri, title: firstLine || resource.uri };
}

/* What a person sent with a message is shown by name: their own text stays
   prose, and an issue they handed over stays a chip, not its whole text. */
export function sentParts(message: ChatMessage): { parts: ChatPart[]; context: SentContext[] } {
    const context = [...(message.context ?? [])];
    if (message.role !== "user") return { parts: message.parts, context };
    const parts: ChatPart[] = [];
    for (const part of message.parts) {
        const embedded = part.kind === "content" ? embeddedContext(part.content) : null;
        if (embedded) {
            context.push(embedded);
            continue;
        }
        if (part.kind !== "text") {
            parts.push(part);
            continue;
        }
        const split = splitSentContext(part.text);
        context.push(...split.context);
        if (split.text) parts.push(split.context.length ? { ...part, text: split.text } : part);
    }
    const shown = context.filter((item) => !isMention(item.uri));
    return { parts, context: shown.filter((item, index) => shown.findIndex((other) => other.uri === item.uri) === index) };
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
    /** Everything folded is thinking, so the turn thought rather than worked. */
    thought: boolean;
}

export const isReply = (part: ChatPart) => part.kind === "text" || part.kind === "content";

/* A page the agent showed and a subagent still at work stay in view when the turn around them folds. */
export const outlivesFold = (part: ChatPart) =>
    (part.kind === "tool" && part.page !== undefined) || (part.kind === "subagent" && part.subagent.state === "running");

/** The parts a message shows: all of them, or once the work before `from` is folded away, the reply and what outlives the fold. */
export function shownParts(parts: ChatPart[], from: number): ChatPart[] {
    return from > 0 ? parts.filter((part, index) => (index >= from && isReply(part)) || outlivesFold(part)) : parts;
}

const replyStart = (parts: ChatPart[], end: number) => {
    let start = end;
    while (start > 0 && isReply(parts[start - 1])) start -= 1;
    return start;
};

const writtenLength = (parts: ChatPart[], start: number, end: number) => {
    let length = 0;
    for (let index = start; index < end; index += 1) {
        const part = parts[index];
        if (part.kind === "text") length += part.text.length;
    }
    return length;
};

/* An agent often answers, makes one more call, such as stopping a server, and
   closes with a line about it. The answer before that call is still the reply. */
function answerBeforeAside(parts: ChatPart[], from: number): number {
    let aside = from;
    while (aside > 0 && !isReply(parts[aside - 1])) aside -= 1;
    if (aside === from || aside === 0) return from;
    const answer = replyStart(parts, aside);
    return writtenLength(parts, answer, aside) > writtenLength(parts, from, parts.length) ? answer : from;
}

/* Runs while a reply streams in, so it walks the turn's parts in place rather than gathering them. */
function workFold(messages: ChatMessage[], start: number, end: number): WorkFold | null {
    const parts = messages[end].parts;
    const closing = replyStart(parts, parts.length);
    if (!parts.slice(closing).some((part) => part.kind === "text")) return null;
    const from = answerBeforeAside(parts, closing);
    let calls = 0;
    let worked = false;
    let acted = false;
    for (let index = start; index <= end; index += 1) {
        const work = messages[index].parts;
        for (let at = 0; at < work.length; at += 1) {
            if (index === end && at >= from && isReply(work[at])) continue;
            if (work[at].kind === "tool") calls += 1;
            worked ||= !isReply(work[at]);
            acted ||= !isReply(work[at]) && work[at].kind !== "thought";
        }
    }
    if (!worked) return null;
    return { id: messages[start].id, start, end, from, calls, took: turnSpan(messages, start, end), thought: !acted };
}

/** How long the turn of assistant messages `start`..`end` took. A prompted turn starts with its prompt; one the
    agent began by itself starts when the core said it did. Null when either end is unknown. */
export function turnSpan(messages: readonly ChatMessage[], start: number, end: number): number | null {
    const before = messages[start - 1];
    const began = (before?.role === "user" ? before.sentAt : undefined) ?? messages[start].opensTurn?.at ?? null;
    const ended = messages[end].endedAt ?? null;
    return began !== null && ended !== null && ended > began ? ended - began : null;
}

const sameFold = (a: WorkFold | undefined, b: WorkFold) =>
    a !== undefined &&
    a.id === b.id &&
    a.start === b.start &&
    a.end === b.end &&
    a.from === b.from &&
    a.calls === b.calls &&
    a.took === b.took &&
    a.thought === b.thought;

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

export const SUBAGENT_WORDS: Readonly<Record<AcpSubagent["state"], string>> = {
    running: "working",
    completed: "done",
    failed: "failed",
    cancelled: "stopped",
    disconnected: "lost",
};

export type SubagentMark = "spinner" | "check" | "warning" | "plug" | "close";

/** The mark beside a subagent's row; each side draws it with its own icon. */
export const SUBAGENT_MARKS: Readonly<Record<AcpSubagent["state"], SubagentMark>> = {
    running: "spinner",
    completed: "check",
    failed: "warning",
    cancelled: "close",
    disconnected: "plug",
};

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

export function subagentsById(messages: ChatMessage[], found = new Map<string, AcpSubagent>()): Map<string, AcpSubagent> {
    for (const message of messages)
        for (const part of message.parts)
            if (part.kind === "subagent") {
                found.set(part.subagent.sessionId, part.subagent);
                subagentsById(part.subagent.messages, found);
            }
    return found;
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
    const detail = [task.summary, task.description, task.lastToolName, task.taskType].find((text) => text && text !== task.name);
    return [task.owner && `from ${task.owner}`, detail].filter(Boolean).join(" · ") || undefined;
}

/* A running agent says what it is doing; one that ended says which phase it was in. */
export function workflowAgentDetail(agent: AcpWorkflowAgent): string | undefined {
    if (agent.state === "failed") return agent.error ?? agent.phase;
    if (agent.state === "running" && agent.activity) return agent.activity;
    return agent.phase;
}

export function workflowAgentCost(agent: AcpWorkflowAgent): string | undefined {
    const cost = [agent.tokens && `${tokenCount(agent.tokens)} tokens`, agent.toolCalls && `${agent.toolCalls} tools`];
    return cost.filter(Boolean).join(" · ") || undefined;
}

export function workflowAgentCount(agents: AcpWorkflowAgent[]): string {
    const finished = agents.filter((agent) => agent.state === "completed" || agent.state === "failed").length;
    return `${finished}/${agents.length} ${agents.length === 1 ? "agent" : "agents"}`;
}

export function subagentCost(subagent: AcpSubagent): string | undefined {
    const calls = subagent.messages.reduce((count, message) => count + message.parts.filter((part) => part.kind === "tool").length, 0);
    const cost = [subagent.tokens && `${tokenCount(subagent.tokens)} tokens`, calls > 0 && callCount(calls)];
    return cost.filter(Boolean).join(" · ") || undefined;
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
