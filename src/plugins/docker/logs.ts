import type { LogLine } from "./api";

/** The most lines a followed log keeps; older ones fall off the top. */
export const MAX_LOG_LINES = 10_000;

export interface LogFilter {
    query: string;
    errorsOnly: boolean;
}

/** New lines added to the end, with the oldest dropped past `max`. */
export function appendLines(lines: readonly LogLine[], more: readonly LogLine[], max = MAX_LOG_LINES): LogLine[] {
    if (more.length === 0) return lines as LogLine[];
    const joined = lines.concat(more);
    return joined.length > max ? joined.slice(joined.length - max) : joined;
}

export function filterLines(lines: readonly LogLine[], filter: LogFilter): LogLine[] {
    const words = filter.query.trim().toLowerCase();
    if (!words && !filter.errorsOnly) return lines as LogLine[];
    return lines.filter((line) => (!filter.errorsOnly || line.stream === "stderr") && (!words || line.text.toLowerCase().includes(words)));
}

/** `2026-10-18T10:00:01.123456789Z` as the clock on this Mac reads it, to the millisecond. */
export function clockOf(at: string | null): string {
    if (!at) return "";
    const date = new Date(at.replace(/(\.\d{3})\d+/, "$1"));
    if (Number.isNaN(date.getTime())) return "";
    return date.toLocaleTimeString(undefined, { hour12: false }) + `.${String(date.getMilliseconds()).padStart(3, "0")}`;
}

export function linesAsText(lines: readonly LogLine[], withTime: boolean): string {
    return lines.map((line) => (withTime && line.at ? `${line.at} ${line.text}` : line.text)).join("\n");
}
