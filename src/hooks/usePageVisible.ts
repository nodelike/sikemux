import { useSyncExternalStore } from "react";

function subscribe(listener: () => void): () => void {
    document.addEventListener("visibilitychange", listener);
    return () => document.removeEventListener("visibilitychange", listener);
}

/** Whether this page is the one on screen, for work that is only worth doing then. */
export function usePageVisible(): boolean {
    return useSyncExternalStore(subscribe, () => !document.hidden);
}
