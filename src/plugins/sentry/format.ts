import type { Exception, Frame, IssueDetail, IssueLevel } from "./api";

export type Tone = "danger" | "warn" | "info" | "idle";

export function levelTone(level: IssueLevel): Tone {
    if (level === "fatal" || level === "error") return "danger";
    if (level === "warning") return "warn";
    if (level === "info") return "info";
    return "idle";
}

/** How long ago an ISO time was, as Sentry writes it: 45s, 12m, 3h, 6d, 4mo. */
export function ago(at: string | null, now = Date.now()): string {
    if (!at) return "—";
    const seconds = Math.max(0, Math.floor((now - Date.parse(at)) / 1000));
    if (Number.isNaN(seconds)) return "—";
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
    if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`;
    if (seconds < 86_400 * 60) return `${Math.floor(seconds / 86_400)}d`;
    return `${Math.floor(seconds / (86_400 * 30))}mo`;
}

/** A count as Sentry shows it: 950, 1.3k, 42k, 1.2m. */
export function count(value: number): string {
    if (value < 1000) return String(value);
    if (value < 10_000) return `${(value / 1000).toFixed(1).replace(/\.0$/, "")}k`;
    if (value < 1_000_000) return `${Math.round(value / 1000)}k`;
    return `${(value / 1_000_000).toFixed(1).replace(/\.0$/, "")}m`;
}

/** The exception that reached the top, which is the one the issue is named for. */
export function topException(detail: Pick<IssueDetail, "event">): Exception | null {
    const exceptions = detail.event?.exceptions ?? [];
    return exceptions[exceptions.length - 1] ?? null;
}

/** Groups library frames that sit together, so a stack shows the app's code and folds the rest. The first frame always shows. */
export function groupFrames(
    frames: Frame[],
): ({ kind: "frame"; frame: Frame; index: number } | { kind: "library"; frames: Frame[]; from: number })[] {
    const groups: ({ kind: "frame"; frame: Frame; index: number } | { kind: "library"; frames: Frame[]; from: number })[] = [];
    frames.forEach((frame, index) => {
        if (frame.inApp || index === 0) {
            groups.push({ kind: "frame", frame, index });
            return;
        }
        const last = groups[groups.length - 1];
        if (last?.kind === "library") last.frames.push(frame);
        else groups.push({ kind: "library", frames: [frame], from: index });
    });
    return groups;
}

export function frameLabel(frame: Frame): string {
    return frame.file ?? frame.module ?? "<unknown>";
}

/** Where a frame's file could be in a project, most likely first, from the paths SDKs report. */
export function projectCandidates(file: string, root: string): string[] {
    const cleaned = file
        .replace(/^[a-z]+:\/\/\/?/i, "")
        .replace(/^webpack-internal:\/\/\//, "")
        .replace(/^\(app-pages-browser\)\//, "")
        .replace(/^~\//, "")
        .replace(/^\.\//, "")
        .replace(/[?#].*$/, "");
    if (!cleaned || cleaned.includes("node_modules/") || cleaned.startsWith("<")) return [];
    const base = root.replace(/\/+$/, "");
    const parts = cleaned.split("/").filter((part) => part && part !== "." && part !== "..");
    if (cleaned.startsWith(`${base}/`)) return [cleaned];
    const candidates: string[] = [];
    for (let skip = 0; skip < parts.length && skip < 6; skip++) {
        const rest = parts.slice(skip);
        if (rest.length === 0) break;
        candidates.push(`${base}/${rest.join("/")}`);
    }
    return candidates;
}
