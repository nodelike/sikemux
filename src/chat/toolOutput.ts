import type { AcpToolCall } from "./types";

export interface ToolOutput {
    text: string;
    /** The output ran past what the transcript keeps, and the rest was let go. */
    cut: boolean;
    exitCode?: number;
    image?: { data: string; mimeType: string };
}

/* A finished call is kept for as long as the session is open, so what it
   printed is kept only up to what a reader would scroll through under a row. */
const MAX_CHARS = 16_000;
const MAX_LINES = 400;
const MAX_IMAGE_CHARS = 1024 * 1024;

function recordOf(value: unknown): Record<string, unknown> | undefined {
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function textOf(value: unknown): string | undefined {
    return typeof value === "string" ? value : undefined;
}

function blockTexts(blocks: unknown): string | undefined {
    if (!Array.isArray(blocks)) return undefined;
    const texts = blocks.map((block) => textOf(recordOf(block)?.text)).filter((text): text is string => text !== undefined);
    return texts.length > 0 ? texts.join("\n") : undefined;
}

/* Claude hands back the tool's own result, a string or a list of blocks.
   Codex wraps a command's in formatted_output, and an MCP call's in result. */
export function outputText(raw: unknown): string | undefined {
    if (typeof raw === "string") return raw;
    const blocks = blockTexts(raw);
    if (blocks !== undefined) return blocks;
    const record = recordOf(raw);
    if (!record) return undefined;
    const direct = textOf(record.formatted_output) ?? textOf(record.output) ?? blockTexts(recordOf(record.result)?.content);
    if (direct !== undefined) return direct;
    const streams = [textOf(record.stdout), textOf(record.stderr)].filter((stream): stream is string => !!stream);
    if (streams.length > 0) return streams.join("\n");
    return textOf(record.error) ?? textOf(recordOf(record.error)?.message);
}

function outputImage(tool: AcpToolCall): ToolOutput["image"] {
    for (const entry of tool.content ?? []) {
        const block = recordOf(recordOf(entry)?.content);
        const data = textOf(block?.data);
        const mimeType = textOf(block?.mimeType);
        if (block?.type === "image" && data && data.length <= MAX_IMAGE_CHARS && mimeType?.startsWith("image/")) return { data, mimeType };
    }
    return undefined;
}

function capped(text: string): { text: string; cut: boolean } {
    let kept = text.replace(/\s+$/, "");
    let cut = false;
    const lines = kept.split("\n");
    if (lines.length > MAX_LINES) {
        kept = lines.slice(0, MAX_LINES).join("\n");
        cut = true;
    }
    if (kept.length > MAX_CHARS) {
        kept = kept.slice(0, MAX_CHARS);
        cut = true;
    }
    return { text: kept, cut };
}

function isMcp(tool: AcpToolCall): boolean {
    return tool.title.startsWith("mcp__");
}

/** What a finished command or MCP call printed. Reads and edits already show what they touched. */
export function toolOutput(tool: AcpToolCall): ToolOutput | null {
    if (tool.kind !== "execute" && !isMcp(tool)) return null;
    const raw = outputText(tool.rawOutput);
    const image = outputImage(tool);
    if (raw === undefined && !image) return null;
    const exitCode = recordOf(tool.rawOutput)?.exit_code;
    return {
        ...capped(raw ?? ""),
        ...(typeof exitCode === "number" ? { exitCode } : {}),
        ...(image ? { image } : {}),
    };
}

/** The line Claude writes beside a shell command to say what it is for. Codex writes none. */
export function toolDescription(tool: AcpToolCall): string | null {
    if (tool.kind !== "execute") return null;
    const description = textOf(recordOf(tool.rawInput)?.description)?.trim();
    return description || null;
}
