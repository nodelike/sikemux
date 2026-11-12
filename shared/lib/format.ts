import { english, type Say } from "./say";

const AGE: [limit: number, unit: string, per: number][] = [
    [3600, "{count}m", 60],
    [86_400, "{count}h", 3600],
    [2_592_000, "{count}d", 86_400],
    [31_536_000, "{count}mo", 2_592_000],
];

function timeOf(at: number | string | null | undefined): number | null {
    if (at === null || at === undefined || at === "") return null;
    const time = typeof at === "number" ? at : Date.parse(at);
    return Number.isFinite(time) ? time : null;
}

/** How long ago, in a tight column: `now`, `5m`, `3h`, `4d`, `2mo`, `1y`. Takes epoch milliseconds or an ISO date. */
export function formatAge(at: number | string | null | undefined, now = Date.now(), say: Say = english): string {
    const time = timeOf(at);
    if (time === null) return "—";
    const seconds = Math.max(0, Math.floor((now - time) / 1000));
    if (seconds < 60) return say("now");
    for (const [limit, unit, per] of AGE) {
        if (seconds < limit) return say(unit, { count: Math.floor(seconds / per) });
    }
    return say("{count}y", { count: Math.floor(seconds / 31_536_000) });
}

/** How long ago, in a sentence: `now`, `5m ago`, `3h ago`. */
export function formatAgo(at: number | string | null | undefined, now = Date.now(), say: Say = english): string {
    const time = timeOf(at);
    if (time === null) return "—";
    if (now - time < 60_000) return say("now");
    return say("{age} ago", { age: formatAge(at, now, say) });
}

/** A countdown in words: `5 min`, `1 hour`, `3 hours`, `2 days`. */
export function formatTimeLeft(until: number, now: number, say: Say = english): string {
    const minutes = Math.max(1, Math.round((until - now) / 60_000));
    if (minutes < 60) return say("{minutes} min", { minutes });
    const hours = Math.round(minutes / 60);
    if (hours < 48) return hours === 1 ? say("1 hour") : say("{hours} hours", { hours });
    return say("{days} days", { days: Math.round(hours / 24) });
}

const BYTE_UNITS = ["KB", "MB", "GB", "TB"];

/** A day the way the apps write one, like 12 November 2026. */
export function formatDay(iso: string, month: "short" | "long" = "long"): string {
    return new Date(iso).toLocaleDateString(undefined, { day: "numeric", month, year: "numeric" });
}

/** Clerk's image host scales the whole picture to fit the size asked for; its crop and cover fits zoom in on a slice of it. */
export function clerkImageUrl(url: string, pixels: number): string {
    return `${url}${url.includes("?") ? "&" : "?"}width=${pixels}&height=${pixels}&fit=scale-down&quality=100`;
}

/** A size: `512 B`, `2.0 KB`, `15 MB`. */
export function formatBytes(bytes: number): string {
    if (Math.abs(bytes) < 1024) return `${bytes} B`;
    let value = bytes / 1024;
    let unit = 0;
    while (Math.abs(value) >= 1024 && unit < BYTE_UNITS.length - 1) {
        value /= 1024;
        unit += 1;
    }
    return `${Math.abs(value) < 10 ? value.toFixed(1) : Math.round(value)} ${BYTE_UNITS[unit]}`;
}

function fieldOf(error: unknown, field: "message" | "category"): string | undefined {
    const value = typeof error === "object" && error !== null ? (error as Record<string, unknown>)[field] : undefined;
    return typeof value === "string" ? value : undefined;
}

/** What a thrown value says went wrong: an Error, a string, or the core's `{ category, message }`. */
export function errorText(error: unknown): string {
    return fieldOf(error, "message") ?? String(error);
}
