import { useEffect } from "react";
import { simApi, type SimStatus } from "../api/sim";
import { externalValue, useExternalValue } from "../lib/externalValue";
import { swallow } from "../state/toast";

const known = externalValue<SimStatus | null>(null);
let asking: Promise<SimStatus> | null = null;

function publish(next: SimStatus): SimStatus {
    known.set(next);
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
    const status = known.get();
    if (status) publish({ ...status, installed: true });
}

export function useSimStatus(): SimStatus | null {
    useEffect(() => {
        if (!known.get()) void loadSimStatus().catch(swallow("read the simulator status"));
    }, []);
    return useExternalValue(known);
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
