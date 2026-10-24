import { safeWebUrl } from "../lib/urls";
import type { ChatRows } from "./rows";
import type { ToolAction, ToolCall } from "./types";

/* The core works out what each call did and to what; these are only the words the app says it in. */
const ACTION_WORDS: Record<ToolAction, string> = {
    read: "read",
    edit: "edit",
    delete: "delete",
    move: "move",
    search: "search",
    execute: "run",
    think: "think",
    fetch: "fetch",
    switch_mode: "mode",
    mcp: "mcp",
    other: "tool",
};

const ACTION_SENTENCES: Partial<Record<ToolAction, string>> = {
    read: "Reading…",
    edit: "Editing…",
    delete: "Deleting…",
    move: "Moving…",
    search: "Searching…",
    execute: "Running a command…",
    think: "Thinking…",
    fetch: "Fetching…",
    switch_mode: "Switching mode…",
};

/* The running row is one line, so it says what the agent is doing rather than quote the call back. */
export function activityLabel(tool: ToolCall): string {
    return tool.row.activity ?? ACTION_SENTENCES[tool.row.action] ?? "Working…";
}

export function toolKind(tool: ToolCall): string {
    return tool.row.verb ?? tool.row.server ?? ACTION_WORDS[tool.row.action];
}

export function toolTarget(tool: ToolCall): string {
    return tool.row.target;
}

export function toolDetail(tool: ToolCall): string | null {
    return tool.row.detail ?? null;
}

export function toolPath(tool: ToolCall): string | null {
    return tool.row.path ?? null;
}

export function toolUrl(target: string): { before: string; raw: string; url: string; after: string } | null {
    const match = /https?:\/\/[^\s<>"'`]+/.exec(target);
    if (!match) return null;
    const raw = match[0].replace(/[.,;:!?)\]]+$/, "");
    const url = safeWebUrl(raw);
    return url ? { before: target.slice(0, match.index), raw, url, after: target.slice(match.index + raw.length) } : null;
}

export function toolRunning(tool: ToolCall): boolean {
    const status = tool.status ?? "pending";
    return status !== "completed" && status !== "failed" && status !== "cancelled";
}

/** What the running call at the end of the chat is doing, in its row's words. */
export function activeToolLabel(chat: Pick<ChatRows, "messages" | "activeTool">): string | null {
    if (!chat.activeTool) return null;
    const part = chat.messages.at(-1)?.parts.find((candidate) => candidate.id === chat.activeTool);
    return part?.kind === "tool" ? activityLabel(part.tool) : null;
}
