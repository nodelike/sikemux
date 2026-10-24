import { tokenCount } from "./contextUsage";
import { toolKind, toolTarget } from "./toolLabels";
import { callCount } from "./toolView";
import type { AsyncTask, ChatPart, ChatRows, SubagentCard, WorkflowAgent } from "./types";

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

export const isReply = (part: ChatPart) => part.kind === "text" || part.kind === "content";

/* A page the agent showed and a subagent still at work stay in view when the turn around them folds. */
export const outlivesFold = (part: ChatPart) =>
    (part.kind === "tool" && part.page !== undefined) || (part.kind === "subagent" && part.subagent.state === "running");

/** The parts a message shows: all of them, or once the work before `from` is folded away, the reply and what outlives the fold. */
export function shownParts(parts: ChatPart[], from: number): ChatPart[] {
    return from > 0 ? parts.filter((part, index) => (index >= from && isReply(part)) || outlivesFold(part)) : parts;
}

export const SUBAGENT_WORDS: Readonly<Record<SubagentCard["state"], string>> = {
    running: "working",
    completed: "done",
    failed: "failed",
    cancelled: "stopped",
    disconnected: "lost",
};

export type SubagentMark = "spinner" | "check" | "warning" | "plug" | "close";

/** The mark beside a subagent's row; each side draws it with its own icon. */
export const SUBAGENT_MARKS: Readonly<Record<SubagentCard["state"], SubagentMark>> = {
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

/* What a subagent is up to: the call it made last says more than the prose it
   wrote before starting. */
export function subagentActivity(subagent: SubagentCard): string {
    if (subagent.lastTool) return `${toolKind(subagent.lastTool)} ${toolTarget(subagent.lastTool)}`.trim();
    return subagentTask(subagent.task);
}

/* A task whose description repeats its name would print the same words twice,
   once in each voice, so the detail takes the first thing that says more. */
export function taskDetail(task: AsyncTask): string | undefined {
    const detail = [task.summary, task.description, task.lastToolName, task.taskType].find((text) => text && text !== task.name);
    return [task.owner && `from ${task.owner}`, detail].filter(Boolean).join(" · ") || undefined;
}

/* A running agent says what it is doing; one that ended says which phase it was in. */
export function workflowAgentDetail(agent: WorkflowAgent): string | undefined {
    if (agent.state === "failed") return agent.error ?? agent.phase;
    if (agent.state === "running" && agent.activity) return agent.activity;
    return agent.phase;
}

export function workflowAgentCost(agent: WorkflowAgent): string | undefined {
    const cost = [agent.tokens && `${tokenCount(agent.tokens)} tokens`, agent.toolCalls && `${agent.toolCalls} tools`];
    return cost.filter(Boolean).join(" · ") || undefined;
}

export function workflowAgentCount(agents: WorkflowAgent[]): string {
    const finished = agents.filter((agent) => agent.state === "completed" || agent.state === "failed").length;
    return `${finished}/${agents.length} ${agents.length === 1 ? "agent" : "agents"}`;
}

export function subagentCost(subagent: SubagentCard): string | undefined {
    const cost = [subagent.tokens && `${tokenCount(subagent.tokens)} tokens`, subagent.calls > 0 && callCount(subagent.calls)];
    return cost.filter(Boolean).join(" · ") || undefined;
}

/* An agent names its own task types — "shell", "monitor" — and they are the
   only thing that separates one background task from another, so they are what
   the groups are cut on. */
export function groupTasks(tasks: AsyncTask[]): [string, AsyncTask[]][] {
    const groups = new Map<string, AsyncTask[]>();
    for (const task of tasks) {
        const kind = task.taskType || "task";
        const existing = groups.get(kind);
        if (existing) existing.push(task);
        else groups.set(kind, [task]);
    }
    return [...groups];
}

/** Every subagent card the chat holds, by its session. */
export function subagentCards(chat: Pick<ChatRows, "messages" | "subagentRows" | "runningSubagents">): Map<string, SubagentCard> {
    const cards = new Map(chat.runningSubagents.map((card) => [card.sessionId, card]));
    for (const rows of [chat.messages, ...Object.values(chat.subagentRows)])
        for (const message of rows) for (const part of message.parts) if (part.kind === "subagent") cards.set(part.subagent.sessionId, part.subagent);
    return cards;
}
