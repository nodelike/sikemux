import { useEffect } from "react";

/** Calls `refresh` every `everyMs` while `active`, so a visible list follows the cluster. */
export function usePoll(active: boolean, everyMs: number, refresh: () => Promise<void>): void {
    useEffect(() => {
        if (!active) return;
        const timer = window.setInterval(() => void refresh().catch(() => {}), everyMs);
        return () => window.clearInterval(timer);
    }, [active, everyMs, refresh]);
}
