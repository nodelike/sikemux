import { safeWebUrl } from "../lib/urls";
import type { ToolAction, ToolCall } from "../wire.generated";

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

export function toolKind(tool: ToolCall): string {
    return tool.row.verb ?? tool.row.server ?? ACTION_WORDS[tool.row.action];
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
