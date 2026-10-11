import type { PageRef } from "../api/pages";
import { outputText } from "./toolOutput";
import { sikemuxToolName } from "./toolRows";
import type { AcpToolCall } from "./types";

const PAGE_MIN_HEIGHT = 48;
const PAGE_MAX_HEIGHT = 2400;
export const PAGE_DEFAULT_HEIGHT = 320;

export function clampPageHeight(height: number): number {
    return Math.min(PAGE_MAX_HEIGHT, Math.max(PAGE_MIN_HEIGHT, Math.ceil(height)));
}

/** The page a finished `page_show` call put in the reply, read from what it printed. */
export function toolPage(tool: AcpToolCall): PageRef | null {
    if (sikemuxToolName(tool.title) !== "page_show" || tool.status !== "completed") return null;
    const printed = outputText(tool.rawOutput) ?? outputText(tool.content?.map((entry) => (entry as { content?: unknown }).content));
    if (!printed) return null;
    let page: unknown;
    try {
        page = (JSON.parse(printed) as { page?: unknown }).page;
    } catch {
        return null;
    }
    if (typeof page !== "object" || page === null) return null;
    const { id, title, height } = page as Record<string, unknown>;
    if (typeof id !== "string" || !/^[0-9a-f]{32}$/.test(id) || typeof title !== "string") return null;
    return { id, title, ...(typeof height === "number" && Number.isFinite(height) ? { height } : {}) };
}

export type PageMessage = { kind: "height"; height: number } | { kind: "link"; url: string } | { kind: "wheel"; deltaY: number };

/** What a framed page asks of the chat: room for its height, a link opened, or the transcript scrolled. */
export function readPageMessage(data: unknown): PageMessage | null {
    if (typeof data !== "object" || data === null) return null;
    const { jsonrpc, method, params } = data as Record<string, unknown>;
    if (jsonrpc !== "2.0" || typeof params !== "object" || params === null) return null;
    const { height, url, deltaY } = params as Record<string, unknown>;
    if (method === "ui/notifications/size-changed" && typeof height === "number" && Number.isFinite(height) && height > 0)
        return { kind: "height", height };
    if (method === "ui/open-link" && typeof url === "string" && /^https?:\/\//i.test(url)) return { kind: "link", url };
    if (method === "sikemux/wheel" && typeof deltaY === "number" && Number.isFinite(deltaY)) return { kind: "wheel", deltaY };
    return null;
}
