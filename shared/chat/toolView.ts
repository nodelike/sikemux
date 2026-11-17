import { english, type Say } from "../lib/say";
import { durationLabel, elapsedLabel } from "./durationLabel";
import { toolKind, toolRunning } from "./toolLabels";
import type { ToolRowIcon } from "./toolRows";
import type { ChatPart, ToolAction, ToolCall } from "../wire.generated";

type ToolPart = Extract<ChatPart, { kind: "tool" }>;

export type ToolIconName = ToolRowIcon | "agent" | "command" | "file" | "move" | "pencil" | "plug" | "trash" | "warning" | "wrench";

const ACTION_ICONS: Partial<Record<ToolAction, ToolIconName>> = {
    mcp: "plug",
    read: "file",
    search: "search",
    edit: "pencil",
    move: "move",
    delete: "trash",
    execute: "command",
    fetch: "globe",
};

/** The icon a call's row shows. Each side draws the name with its own icon. */
export function toolIconName(tool: ToolCall): ToolIconName {
    if (tool.status === "failed") return "warning";
    return (tool.row.icon as ToolIconName | undefined) ?? ACTION_ICONS[tool.row.action] ?? "agent";
}

/* A call the turn cut off has a duration, but printing it would read as a call
   that ran that long and then finished, so it says why it stopped. */
export function toolElapsed(part: ToolPart, timed = true, say: Say = english): string | null {
    if (part.tool.status === "cancelled") return say("stopped");
    return timed && part.startedAt !== undefined && part.endedAt !== undefined ? durationLabel(part.endedAt - part.startedAt, say) : null;
}

/** Lines of a call's output shown before "Show all". */
export const OUTPUT_FOLD_LINES = 12;

export const callsLabel = (count: number, say: Say = english): string => (count === 1 ? say("1 tool call") : say("{count} tool calls", { count }));

export const callCount = (count: number, say: Say = english): string => (count === 1 ? say("1 call") : say("{count} calls", { count }));

interface ToolGroupSummary {
    current: ToolPart | undefined;
    /** While a call runs, what the agent said it is for. */
    said: string | null;
    /** The time the run's finished calls took. */
    spent: number;
    /** Characters in the longest call name, for lining the names up in one column. */
    kindWidth: number;
}

export function toolGroupSummary(tools: readonly ToolPart[], say: Say = english): ToolGroupSummary {
    const current = tools.find((part) => toolRunning(part.tool));
    const spent = tools.reduce(
        (total, part) => total + (part.startedAt !== undefined && part.endedAt !== undefined ? part.endedAt - part.startedAt : 0),
        0,
    );
    return {
        current,
        said: current?.tool.description ?? null,
        spent,
        kindWidth: Math.min(16, Math.max(4, ...tools.map((part) => toolKind(part.tool, say).length))),
    };
}

/** A folded turn's line: whether it worked or only thought, and for how long. */
export function workedLabel(took: number | null, thought: boolean, say: Say = english): string {
    if (took === null) return thought ? say("Thought") : say("Worked");
    const duration = durationLabel(took, say);
    return thought ? say("Thought for {duration}", { duration }) : say("Worked for {duration}", { duration });
}

/** Whole seconds since `started`, on top of `spent` already counted. */
export function secondsSince(started: number, now: number, spent = 0, say: Say = english): string {
    return elapsedLabel(Math.max(0, Math.floor((spent + now - started) / 1000)), say);
}
