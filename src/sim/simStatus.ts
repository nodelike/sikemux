import { useEffect, useSyncExternalStore } from "react";
import { simApi, type SimStatus } from "../api/sim";

let known: SimStatus | null = null;
let asking: Promise<SimStatus> | null = null;
const listeners = new Set<() => void>();

function publish(next: SimStatus): SimStatus {
    known = next;
    for (const listener of listeners) listener();
    return next;
}

/** The simulator's status, asked once per run and shared by everything that shows the simulator. */
export function loadSimStatus(): Promise<SimStatus> {
    asking ??= simApi.status().then(publish, (error: unknown) => {
        asking = null;
        throw error;
    });
    return asking;
}

/** Whether the simulator can run here, with nothing in the way such as a missing Xcode. */
export const simUsable = (status: SimStatus | null): boolean => !!status?.supported && !status.reason;

export function markSimInstalled(): void {
    if (known) publish({ ...known, installed: true });
}

export function useSimStatus(): SimStatus | null {
    const status = useSyncExternalStore(
        (listener) => {
            listeners.add(listener);
            return () => listeners.delete(listener);
        },
        () => known,
    );
    useEffect(() => {
        if (!known) void loadSimStatus().catch(() => {});
    }, []);
    return status;
}

let preparing: Promise<void> | null = null;

/** Gets the helper ready once, however many panes are waiting for it. */
export function prepareSim(): Promise<void> {
    preparing ??= simApi
        .prepare()
        .then(markSimInstalled)
        .finally(() => {
            preparing = null;
        });
    return preparing;
}
