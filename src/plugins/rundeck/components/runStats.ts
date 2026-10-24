import type { RundeckRun } from "../types.generated";

/** How long a run took, or has taken so far while it goes on. */
export function runSecs(run: RundeckRun, now: number): number {
    if (run.secs !== null) return run.secs;
    const start = Date.parse(run.startedAt ?? "");
    return Number.isNaN(start) ? 0 : Math.max(0, (now - start) / 1000);
}

export function byDay(runs: RundeckRun[]): { day: string; runs: RundeckRun[] }[] {
    const days: { day: string; runs: RundeckRun[] }[] = [];
    for (const ex of runs) {
        const at = Date.parse(ex.startedAt ?? "");
        const day = Number.isNaN(at)
            ? "Unknown date"
            : new Date(at).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
        const last = days[days.length - 1];
        if (last?.day === day) last.runs.push(ex);
        else days.push({ day, runs: [ex] });
    }
    return days;
}

export function clockTime(iso: string | null): string {
    const at = Date.parse(iso ?? "");
    return Number.isNaN(at) ? "" : new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}
