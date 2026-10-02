import { basename } from "../lib/paths";
import { safeWebUrl } from "../terminal/interactions";
import type { AcpToolCall, ChatMessage } from "./types";

// Splits `mcp__server__tool` so the server name can be de-emphasized.
export function toolLabel(title: string): { scope?: string; name: string } {
    const segments = title.split("__");
    return segments[0] === "mcp" && segments.length > 2 ? { scope: segments[1], name: segments.slice(2).join("__") } : { name: title };
}

const ACTIVITY_BY_KIND: Record<string, string> = {
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

/* A tool titles itself with what it was handed — often a whole shell command.
   The running row is one line, so say what the agent is doing rather than
   quote it back. */
export function activityLabel(tool: AcpToolCall): string {
    const byKind = ACTIVITY_BY_KIND[tool.kind ?? ""];
    if (byKind) return byKind;
    const name = toolLabel(tool.title).name.split("\n")[0].trim();
    return name.length > 0 && name.length <= 40 ? name : "Working…";
}

const KIND_WORDS: Record<string, string> = {
    read: "read",
    edit: "edit",
    delete: "delete",
    move: "move",
    search: "search",
    execute: "run",
    think: "think",
    fetch: "fetch",
    switch_mode: "mode",
};

export function toolKind(tool: AcpToolCall): string {
    const byKind = KIND_WORDS[tool.kind ?? ""];
    if (byKind) return byKind;
    const { scope, name } = toolLabel(tool.title);
    return scope ?? name.split(/[\s(]/)[0].slice(0, 12).toLowerCase();
}

/* The row has one line for the target, so a path shows the name it ends in and
   keeps the rest in the tooltip. A command is not a path and stays as typed. */
export function toolTarget(tool: AcpToolCall): string {
    const line = toolLabel(tool.title).name.split("\n")[0].trim();
    if (!line.includes("/") || /\s/.test(line) || safeWebUrl(line)) return line;
    return basename(line) || line;
}

export function toolUrl(target: string): { before: string; raw: string; url: string; after: string } | null {
    const match = /https?:\/\/[^\s<>"'`]+/.exec(target);
    if (!match) return null;
    const raw = match[0].replace(/[.,;:!?)\]]+$/, "");
    const url = safeWebUrl(raw);
    return url ? { before: target.slice(0, match.index), raw, url, after: target.slice(match.index + raw.length) } : null;
}

/* Which file a call was about: the one it reported touching, or the one its
   title names when it reported nothing. A shell command is not a file, and a
   title with a space in it is a command. */
export function toolPath(tool: AcpToolCall): string | null {
    const first = Array.isArray(tool.locations) ? tool.locations[0] : null;
    if (first && typeof first === "object") {
        const { path, line } = first as { path?: unknown; line?: unknown };
        if (typeof path === "string" && path) return typeof line === "number" ? `${path}:${line}` : path;
    }
    const named = toolLabel(tool.title).name.split("\n")[0].trim();
    return named.includes("/") && !/\s/.test(named) && !safeWebUrl(named) ? named : null;
}

export function toolRunning(tool: AcpToolCall): boolean {
    const status = tool.status ?? "pending";
    return status !== "completed" && status !== "failed" && status !== "cancelled";
}

export function activeToolLabel(messages: ChatMessage[]): string | null {
    const parts = messages.at(-1)?.parts ?? [];
    for (let index = parts.length - 1; index >= 0; index -= 1) {
        const part = parts[index];
        if (part.kind !== "tool") continue;
        return toolRunning(part.tool) ? activityLabel(part.tool) : null;
    }
    return null;
}
