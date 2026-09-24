export const DEFAULT_PROD_ENVS = ["prod", "production", "prd", "live"];
export const DEFAULT_BRANCH_OPTIONS = ["BRANCH", "GIT_BRANCH", "GIT_REF", "REF"];

/** The key of the first option named in `branchOptions`, matched case-insensitively and tried in order. */
export function branchOptionName(names: Iterable<string>, branchOptions: string[]): string | null {
    const list = [...names];
    for (const wanted of branchOptions) {
        const lower = wanted.toLowerCase();
        const hit = list.find((name) => name.toLowerCase() === lower);
        if (hit !== undefined) return hit;
    }
    return null;
}

export function branchOf(options: Record<string, string> | null | undefined, branchOptions: string[]): string | null {
    if (!options) return null;
    const key = branchOptionName(Object.keys(options), branchOptions);
    if (key === null) return null;
    return options[key] || null;
}

export function groupSegments(group: string | null | undefined): string[] {
    return (group ?? "")
        .split("/")
        .map((segment) => segment.trim())
        .filter(Boolean);
}

export function topFolder(group: string | null | undefined): string | null {
    return groupSegments(group)[0] ?? null;
}

export function envOf(project: string, group: string | null | undefined): string {
    return topFolder(group) ?? project;
}

function envTokens(name: string): string[] {
    const lower = name.toLowerCase();
    return [lower, ...lower.split(/[-_./\s]+/).filter(Boolean)];
}

export function isProdEnv(env: string | null | undefined, prodEnvs: string[]): boolean {
    if (!env) return false;
    const wanted = new Set(prodEnvs.map((entry) => entry.trim().toLowerCase()).filter(Boolean));
    return envTokens(env).some((token) => wanted.has(token));
}

export function isProdTarget(project: string, group: string | null | undefined, prodEnvs: string[]): boolean {
    return isProdEnv(topFolder(group), prodEnvs) || isProdEnv(project, prodEnvs);
}

export type EnvTone = "prod" | "staging" | "preprod" | "dev" | "other";

/** Colour for an environment name; production is decided by `isProdTarget`, never here. */
export function envTone(name: string | null | undefined): Exclude<EnvTone, "prod"> {
    const tokens = envTokens(name ?? "");
    if (tokens.some((t) => t.startsWith("stag") || t === "stg" || t === "qa" || t === "uat")) return "staging";
    if (tokens.some((t) => t.startsWith("pre"))) return "preprod";
    if (tokens.some((t) => t.startsWith("dev") || t === "local" || t === "test")) return "dev";
    return "other";
}

export function targetTone(project: string, group: string | null | undefined, prodEnvs: string[]): EnvTone {
    return isProdTarget(project, group, prodEnvs) ? "prod" : envTone(envOf(project, group));
}

export function inGroup(jobGroup: string | null | undefined, activeGroup: string | null | undefined): boolean {
    if (!activeGroup) return true;
    const path = groupSegments(jobGroup).join("/");
    const active = groupSegments(activeGroup).join("/");
    return path === active || path.startsWith(`${active}/`);
}

/** The group segment directly below `activeGroup`, or null when the job sits in `activeGroup` itself. */
export function childSegment(jobGroup: string | null | undefined, activeGroup: string | null | undefined): string | null {
    const depth = groupSegments(activeGroup).length;
    return groupSegments(jobGroup)[depth] ?? null;
}

export function qualifiedName(name: string, group: string | null | undefined): string {
    const path = groupSegments(group).join("/");
    return path ? `${path}/${name}` : name;
}

const LIVE_STATUSES = new Set(["running", "scheduled", "queued"]);

export function isLiveStatus(status: string | null | undefined): boolean {
    return LIVE_STATUSES.has((status ?? "").toLowerCase());
}

export function displayStatus(status: string | null | undefined, customStatus: string | null | undefined): string {
    if (customStatus && (!status || status.toLowerCase() === "other")) return customStatus;
    return status ?? "unknown";
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

export function relativeTime(iso: string, now = Date.now()): string {
    const t = Date.parse(iso);
    if (Number.isNaN(t)) return "";
    const dt = (now - t) / 1000;
    if (dt < 5) return "just now";
    if (dt < 60) return `${Math.floor(dt)}s ago`;
    if (dt < 3600) return `${Math.floor(dt / 60)}m ago`;
    if (dt < 86400) return `${Math.floor(dt / 3600)}h ago`;
    return `${Math.floor(dt / 86400)}d ago`;
}

export function duration(start: string | null, end: string | null, now = Date.now()): string {
    if (!start) return "";
    const a = Date.parse(start);
    const b = end ? Date.parse(end) : now;
    if (Number.isNaN(a) || Number.isNaN(b)) return "";
    const s = Math.max(0, Math.round((b - a) / 1000));
    if (s < 60) return `${s}s`;
    const m = Math.floor(s / 60);
    const rest = s % 60;
    if (m < 60) return rest ? `${m}m ${rest}s` : `${m}m`;
    const h = Math.floor(m / 60);
    return `${h}h ${m % 60}m`;
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
