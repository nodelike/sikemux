import { useSyncExternalStore } from "react";
import type { PriceOverride } from "../api/activity";

const KEY = "sikemux.activity.prices";
const listeners = new Set<() => void>();
let current: Record<string, PriceOverride> = read();

function read(): Record<string, PriceOverride> {
    try {
        const parsed: unknown = JSON.parse(localStorage.getItem(KEY) ?? "{}");
        if (!parsed || typeof parsed !== "object") return {};
        const valid: Record<string, PriceOverride> = {};
        for (const [model, price] of Object.entries(parsed as Record<string, Partial<PriceOverride>>)) {
            if (Number.isFinite(price?.input) && Number.isFinite(price?.output)) valid[model] = price as PriceOverride;
        }
        return valid;
    } catch {
        return {};
    }
}

function write(next: Record<string, PriceOverride>) {
    current = next;
    localStorage.setItem(KEY, JSON.stringify(next));
    for (const listener of listeners) listener();
}

export function setPriceOverride(model: string, price: PriceOverride | null) {
    const next = { ...current };
    if (price) next[model] = price;
    else delete next[model];
    write(next);
}

function subscribe(listener: () => void) {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

export function usePriceOverrides(): Record<string, PriceOverride> {
    return useSyncExternalStore(subscribe, () => current);
}
