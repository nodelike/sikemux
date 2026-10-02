import type { Billable } from "../api";

export function billedMinutes(billable: readonly Billable[]): number {
    return billable.reduce((total, each) => total + Math.ceil(each.totalMs / 60_000), 0);
}
