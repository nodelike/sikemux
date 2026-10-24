import { formatDuration } from "../../plugin-api/format";

export const DEFAULT_PROD_ENVS = ["prod", "production", "prd", "live"];
export const DEFAULT_BRANCH_OPTIONS = ["BRANCH", "GIT_BRANCH", "GIT_REF", "REF"];

export function groupSegments(group: string | null | undefined): string[] {
    return (group ?? "")
        .split("/")
        .map((segment) => segment.trim())
        .filter(Boolean);
}

const LIVE_STATUSES = new Set(["running", "scheduled", "queued"]);

export function isLiveStatus(status: string | null | undefined): boolean {
    return LIVE_STATUSES.has((status ?? "").toLowerCase());
}

export function splitList(text: string): string[] {
    return text
        .split(",")
        .map((part) => part.trim())
        .filter(Boolean);
}

export function basenameOf(path: string): string {
    return path.replace(/\/+$/, "").split("/").pop() ?? "";
}

export function duration(start: string | null, end: string | null, now = Date.now()): string {
    if (!start) return "";
    const a = Date.parse(start);
    const b = end ? Date.parse(end) : now;
    if (Number.isNaN(a) || Number.isNaN(b)) return "";
    return formatDuration(Math.max(0, b - a));
}

export function formatTime(iso: string, withSeconds = false): string {
    const t = Date.parse(iso);
    if (Number.isNaN(t)) return iso;
    return new Date(t).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
        ...(withSeconds ? { second: "2-digit" as const } : {}),
    });
}

/** A `datetime-local` value as ISO 8601 with the machine's UTC offset, the form Rundeck's runAtTime wants. */
export function localDateTimeToIso(value: string): string | null {
    const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(value);
    if (!match) return null;
    const [, y, mo, d, h, mi, s = "00"] = match;
    const date = new Date(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi), Number(s));
    if (Number.isNaN(date.getTime())) return null;
    const offset = -date.getTimezoneOffset();
    const sign = offset >= 0 ? "+" : "-";
    const abs = Math.abs(offset);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${y}-${mo}-${d}T${h}:${mi}:${s}${sign}${pad(Math.floor(abs / 60))}:${pad(abs % 60)}`;
}
