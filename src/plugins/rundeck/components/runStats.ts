import type { RundeckExecution } from "../api";
import { isLiveStatus } from "../shape";

export const startedAt = (ex: RundeckExecution): string | null => ex["date-started"]?.date ?? null;
export const endedAt = (ex: RundeckExecution): string | null => ex["date-ended"]?.date ?? null;

export function runSeconds(ex: RundeckExecution, now: number): number {
    const start = Date.parse(startedAt(ex) ?? "");
    const end = endedAt(ex) ? Date.parse(endedAt(ex)!) : now;
    return Number.isNaN(start) || Number.isNaN(end) ? 0 : Math.max(0, (end - start) / 1000);
}

/** The median time of the runs that succeeded, so one stuck or instant run doesn't skew it. */
export function usualSeconds(executions: RundeckExecution[], now: number): number | null {
    const times = executions
        .filter((ex) => ex.status === "succeeded")
        .map((ex) => runSeconds(ex, now))
        .sort((a, b) => a - b);
    return times.length ? times[Math.floor(times.length / 2)] : null;
}

export function fastestAndSlowest(executions: RundeckExecution[], now: number): [number, number] | null {
    const times = executions.filter((ex) => ex.status === "succeeded").map((ex) => runSeconds(ex, now));
    return times.length ? [Math.min(...times), Math.max(...times)] : null;
}

export const liveRun = (executions: RundeckExecution[]): RundeckExecution | null => executions.find((ex) => isLiveStatus(ex.status)) ?? null;

export function byDay(executions: RundeckExecution[]): { day: string; runs: RundeckExecution[] }[] {
    const days: { day: string; runs: RundeckExecution[] }[] = [];
    for (const ex of executions) {
        const at = Date.parse(startedAt(ex) ?? "");
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
