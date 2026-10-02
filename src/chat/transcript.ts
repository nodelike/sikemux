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

export function groupParts(parts: ChatPart[]): PartGroup[] {
    const groups: PartGroup[] = [];
    for (const part of parts) {
        const last = groups.at(-1);
        if (part.kind !== "tool") groups.push({ id: part.id, part });
        else if (last && "tools" in last) last.tools.push(part);
        else groups.push({ id: part.id, tools: [part] });
    }
    return groups;
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
