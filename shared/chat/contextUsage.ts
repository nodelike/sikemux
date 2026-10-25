import type { ContextUsage } from "./types";

export type ContextTone = "steady" | "warm" | "hot";

/** How much of the context window is used, from 0 to 1. */
export function contextShare(usage: ContextUsage): number {
    return Math.min(1, Math.max(0, usage.used / usage.size));
}

/** The agent's own colour while there is room, warm from 70% and hot from 90%. */
export function contextTone(usage: ContextUsage): ContextTone {
    const share = contextShare(usage);
    if (share >= 0.9) return "hot";
    if (share >= 0.7) return "warm";
    return "steady";
}

const compact = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });

export const tokenCount = (tokens: number): string => compact.format(tokens);

/* An adapter names its own currency, and Intl throws on a code it does not know. */
export function money({ amount, currency }: NonNullable<ContextUsage["cost"]>): string {
    try {
        return new Intl.NumberFormat("en", { style: "currency", currency, maximumFractionDigits: 2 }).format(amount);
    } catch {
        return `${amount.toFixed(2)} ${currency}`;
    }
}
