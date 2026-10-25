const AGE: [limit: number, unit: string, per: number][] = [
    [3600, "m", 60],
    [86_400, "h", 3600],
    [2_592_000, "d", 86_400],
    [31_536_000, "mo", 2_592_000],
];

function timeOf(at: number | string | null | undefined): number | null {
    if (at === null || at === undefined || at === "") return null;
    const time = typeof at === "number" ? at : Date.parse(at);
    return Number.isFinite(time) ? time : null;
}

/** How long ago, in a tight column: `now`, `5m`, `3h`, `4d`, `2mo`, `1y`. Takes epoch milliseconds or an ISO date. */
export function formatAge(at: number | string | null | undefined, now = Date.now()): string {
    const time = timeOf(at);
    if (time === null) return "—";
    const seconds = Math.max(0, Math.floor((now - time) / 1000));
    if (seconds < 60) return "now";
    for (const [limit, unit, per] of AGE) {
        if (seconds < limit) return `${Math.floor(seconds / per)}${unit}`;
    }
    return `${Math.floor(seconds / 31_536_000)}y`;
}

/** How long ago, in a sentence: `now`, `5m ago`, `3h ago`. */
export function formatAgo(at: number | string | null | undefined, now = Date.now()): string {
    const age = formatAge(at, now);
    return age === "now" || age === "—" ? age : `${age} ago`;
}

/** A span of time: `350ms`, `2.4s`, `45s`, `2m 5s`, `1h 5m`. */
export function formatDuration(ms: number | null | undefined): string {
    if (ms === null || ms === undefined || !Number.isFinite(ms)) return "—";
    if (ms <= 0) return "0s";
    if (ms < 1) return `${Math.round(ms * 1000)}µs`;
    if (ms < 1000) return `${Number(ms.toPrecision(3))}ms`;
    if (ms < 10_000) return `${Number((ms / 1000).toFixed(1))}s`;
    const seconds = Math.round(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return seconds % 60 ? `${minutes}m ${seconds % 60}s` : `${minutes}m`;
    const hours = Math.floor(minutes / 60);
    return minutes % 60 ? `${hours}h ${minutes % 60}m` : `${hours}h`;
}

/** A countdown in words: `5 min`, `1 hour`, `3 hours`, `2 days`. */
export function formatTimeLeft(until: number, now: number): string {
    const minutes = Math.max(1, Math.round((until - now) / 60_000));
    if (minutes < 60) return `${minutes} min`;
    const hours = Math.round(minutes / 60);
    if (hours < 48) return hours === 1 ? "1 hour" : `${hours} hours`;
    return `${Math.round(hours / 24)} days`;
}

/** Seconds as a clock: `1:52`, `1:02:05`. */
export function formatClock(seconds: number): string {
    const whole = Math.round(seconds);
    const h = Math.floor(whole / 3600);
    const m = Math.floor((whole % 3600) / 60);
    const s = String(whole % 60).padStart(2, "0");
    return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** `0.4.0-nightly.10` as the release `0.4.0` and the channel `nightly 10`. */
export function splitVersion(version: string): { release: string; channel: string | null } {
    const match = /^(\d+\.\d+\.\d+)-([a-z]+)(?:\.(\d+))?$/i.exec(version);
    if (!match) return { release: version, channel: null };
    const [, release, name, build] = match;
    return { release, channel: build ? `${name} ${build}` : name };
}

const BYTE_UNITS = ["KB", "MB", "GB", "TB"];

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

/** The kind of failure the core reported, such as `file-conflict`. */
export function errorCategory(error: unknown): string | undefined {
    return fieldOf(error, "category");
}
