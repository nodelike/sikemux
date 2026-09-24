import type { ServiceHealth } from "./api";
import type { ServiceSort } from "./state";

export interface ServiceRow {
    service: string;
    calls: number;
    errors: number;
    errorRate: number;
    p99Ms: number;
}

/** One row per service. Across environments the counts add up and the worst p99 stands. */
export function mergeByService(rows: readonly ServiceHealth[], environment: string | null): ServiceRow[] {
    const merged = new Map<string, ServiceRow>();
    for (const row of rows) {
        if (environment !== null && row.environment !== environment) continue;
        const current = merged.get(row.service);
        if (!current) {
            merged.set(row.service, { service: row.service, calls: row.calls, errors: row.errors, errorRate: row.errorRate, p99Ms: row.p99Ms });
            continue;
        }
        current.calls += row.calls;
        current.errors += row.errors;
        current.errorRate = current.calls === 0 ? 0 : current.errors / current.calls;
        current.p99Ms = Math.max(current.p99Ms, row.p99Ms);
    }
    return [...merged.values()];
}

export const SORTERS: Record<ServiceSort, (left: ServiceRow, right: ServiceRow) => number> = {
    errors: (left, right) => right.errorRate - left.errorRate || right.errors - left.errors || right.calls - left.calls,
    calls: (left, right) => right.calls - left.calls,
    p99: (left, right) => right.p99Ms - left.p99Ms,
    name: (left, right) => left.service.localeCompare(right.service),
};

export function percent(rate: number): string {
    if (rate === 0) return "0%";
    if (rate < 0.001) return "<0.1%";
    return `${(rate * 100).toFixed(rate >= 0.1 ? 0 : 1)}%`;
}

export function perMinute(count: number, minutes: number): string {
    const rate = count / Math.max(1, minutes);
    if (rate >= 10_000) return `${(rate / 1_000).toFixed(0)}k/min`;
    if (rate >= 1_000) return `${(rate / 1_000).toFixed(1)}k/min`;
    if (rate >= 10) return `${Math.round(rate)}/min`;
    return `${Number(rate.toPrecision(2))}/min`;
}
