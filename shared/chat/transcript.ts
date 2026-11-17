import { english, type Say } from "../lib/say";
import { tokenCount } from "./contextUsage";
import { toolKind } from "./toolLabels";
import { callCount } from "./toolView";
import type { AsyncTask, ChatPart, SubagentCard, WorkflowAgent } from "../wire.generated";

const MAX_DETAIL_CHARS = 120_000;

export function formatDetail(value: unknown, say: Say = english): string {
    let formatted: string;
    try {
        formatted = JSON.stringify(value, null, 2) ?? String(value);
    } catch {
        formatted = String(value);
    }
    return formatted.length > MAX_DETAIL_CHARS ? `${formatted.slice(0, MAX_DETAIL_CHARS)}\n${say("… output truncated")}` : formatted;
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

/* Consecutive calls share one group, except a page or GIF the agent showed, which draws in
   the call's place and is part of its reply rather than a step towards it. */
export function groupParts(parts: ChatPart[]): PartGroup[] {
    const groups: PartGroup[] = [];
    for (const part of parts) {
        const last = groups.at(-1);
        if (part.kind !== "tool" || part.page || part.gif) groups.push({ id: part.id, part });
        else if (last && "tools" in last) last.tools.push(part);
        else groups.push({ id: part.id, tools: [part] });
    }
    return groups;
}

/** The parts a message shows: all of them, or once its turn's work is folded away, those the core keeps in view. */
export function shownParts(parts: ChatPart[], shown: readonly number[] | null): ChatPart[] {
    return shown === null ? parts : parts.filter((_, index) => shown.includes(index));
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
export function subagentActivity(subagent: SubagentCard, say: Say = english): string {
    if (subagent.lastTool) return `${toolKind(subagent.lastTool, say)} ${subagent.lastTool.row.target}`.trim();
    return subagentTask(subagent.task);
}

/* A task whose description repeats its name would print the same words twice,
   once in each voice, so the detail takes the first thing that says more. */
export function taskDetail(task: AsyncTask, say: Say = english): string | undefined {
    const detail = [task.summary, task.description, task.lastToolName, task.taskType].find((text) => text && text !== task.name);
    return [task.owner && say("from {owner}", { owner: task.owner }), detail].filter(Boolean).join(" · ") || undefined;
}

/* A running agent says what it is doing; one that ended says which phase it was in. */
export function workflowAgentDetail(agent: WorkflowAgent): string | undefined {
    if (agent.state === "failed") return agent.error ?? agent.phase;
    if (agent.state === "running" && agent.activity) return agent.activity;
    return agent.phase;
}

export function workflowAgentCost(agent: WorkflowAgent, say: Say = english): string | undefined {
    const cost = [
        agent.tokens && say("{count} tokens", { count: tokenCount(agent.tokens) }),
        agent.toolCalls && say("{count} tools", { count: agent.toolCalls }),
    ];
    return cost.filter(Boolean).join(" · ") || undefined;
}

export function workflowAgentCount(agents: WorkflowAgent[], say: Say = english): string {
    const finished = agents.filter((agent) => agent.state === "completed" || agent.state === "failed").length;
    return agents.length === 1 ? say("{finished}/1 agent", { finished }) : say("{finished}/{count} agents", { finished, count: agents.length });
}

export function subagentCost(subagent: SubagentCard, say: Say = english): string | undefined {
    const cost = [
        subagent.tokens && say("{count} tokens", { count: tokenCount(subagent.tokens) }),
        subagent.calls > 0 && callCount(subagent.calls, say),
    ];
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
