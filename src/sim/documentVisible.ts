import { useSyncExternalStore } from "react";

const subscribe = (listener: () => void) => {
    document.addEventListener("visibilitychange", listener);
    return () => document.removeEventListener("visibilitychange", listener);
};

/** False while the app's window is hidden or minimised. */
export function useDocumentVisible(): boolean {
    return useSyncExternalStore(subscribe, () => document.visibilityState !== "hidden");
}
