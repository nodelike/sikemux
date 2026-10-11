import { basename } from "../lib/paths";
import { durationLabel } from "./durationLabel";
import { TOOL_ROWS } from "./generated/toolRows";
import type { AcpToolCall } from "./types";

export type ToolRowIcon =
    | "activity"
    | "book"
    | "camera"
    | "click"
    | "clock"
    | "close"
    | "code"
    | "dashboard"
    | "download"
    | "eye"
    | "folder"
    | "globe"
    | "highlighter"
    | "home-button"
    | "hourglass"
    | "issue"
    | "keyboard"
    | "link"
    | "log-lines"
    | "message"
    | "mouse"
    | "network"
    | "panel"
    | "phone"
    | "pinch"
    | "pointer"
    | "pull-request"
    | "record"
    | "refresh"
    | "resize"
    | "rocket"
    | "rotate"
    | "run"
    | "search"
    | "square-plus"
    | "status"
    | "steps"
    | "stop"
    | "swipe"
    | "tabs"
    | "tap"
    | "terminal"
    | "text"
    | "touch-path"
    | "unlink"
    | "upload"
    | "waterfall"
    | "window"
    | "workflow";

// The kinds the agent protocol gives its own calls, so ours take the same colours.
export type ToolRowKind = "read" | "search" | "fetch" | "edit" | "move" | "delete" | "execute" | "think";

/* Each tool's row is declared beside its schema, in browser/tools.json or a
   plugin's manifest. The first template whose every {argument} the call was
   given becomes the target; a template with none always matches. */
export interface ToolRowSpec {
    verb: string;
    kind: ToolRowKind;
    icon: ToolRowIcon;
    target: readonly string[];
    detail?: readonly string[];
    /** The plugin whose tool it is; its row shows that plugin's mark. */
    plugin?: string;
}

export interface ToolRowText {
    verb: string;
    kind: ToolRowKind;
    icon: ToolRowIcon;
    target: string;
    detail: string | null;
    plugin: string | null;
}

const SIKEMUX_TOOL = /sikemux-tools(?:__|[./:]\s*)([a-z][a-z0-9_]*)/;

/** The tool's name without the server hosts put in front of it. */
export function sikemuxToolName(title: string): string {
    return SIKEMUX_TOOL.exec(title)?.[1] ?? title.trim();
}

function rowSpec(title: string): ToolRowSpec | null {
    const name = sikemuxToolName(title);
    return Object.hasOwn(TOOL_ROWS, name) ? TOOL_ROWS[name as keyof typeof TOOL_ROWS] : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

// Some agents hand over the server and tool beside the arguments rather than the arguments alone.
export function toolArguments(tool: AcpToolCall): Record<string, unknown> {
    const input = tool.rawInput;
    if (!isRecord(input)) return {};
    return isRecord(input.arguments) ? input.arguments : input;
}

function valueText(key: string, value: unknown): string | null {
    const shorten = (text: string) => (/path/i.test(key) ? basename(text) : text);
    if (typeof value === "string") {
        const line = value.split("\n")[0].trim();
        return line ? shorten(line) : null;
    }
    if (typeof value === "number" && Number.isFinite(value)) return /(^ms|Ms)$/.test(key) ? durationLabel(value) : String(value);
    if (!Array.isArray(value) || value.length === 0) return null;
    if (value.every((item) => typeof item === "string")) return value.map(shorten).join(", ");
    return String(value.length);
}

function fill(templates: readonly string[] | undefined, args: Record<string, unknown>): string | null {
    for (const template of templates ?? []) {
        let complete = true;
        const text = template.replace(/\{(\w+)\}/g, (_, key: string) => {
            const value = valueText(key, args[key]);
            if (value === null) complete = false;
            return value ?? "";
        });
        if (complete) return text;
    }
    return null;
}

export function sikemuxToolRow(tool: AcpToolCall): ToolRowText | null {
    const spec = rowSpec(tool.title);
    if (!spec) return null;
    const args = toolArguments(tool);
    const target = fill(spec.target, args) ?? "";
    const detail = fill(spec.detail, args);
    return {
        verb: spec.verb,
        kind: spec.kind,
        icon: spec.icon,
        target,
        detail: detail && detail !== target ? detail : null,
        plugin: spec.plugin ?? null,
    };
}

/* A finished call lets go of what it was handed, so the arguments its row
   names are kept as the short text the row shows. */
export function rowArguments(tool: AcpToolCall): Record<string, string> | null {
    const spec = rowSpec(tool.title);
    if (!spec) return null;
    const args = toolArguments(tool);
    const kept: Record<string, string> = {};
    for (const template of [...spec.target, ...(spec.detail ?? [])]) {
        for (const [, key] of template.matchAll(/\{(\w+)\}/g)) {
            const value = valueText(key, args[key]);
            if (value !== null) kept[key] = value;
        }
    }
    return kept;
}
