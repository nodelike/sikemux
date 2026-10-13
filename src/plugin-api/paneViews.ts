import { create } from "zustand";
import { onPaneClosed } from "./host";

export interface PaneViews<V> {
    use(paneId: string): V;
    get(paneId: string): V;
    update(paneId: string, change: Partial<V>): void;
}

/**
 * What each pane of a plugin's surface shows, starting from `initial` and forgotten when the pane closes.
 * `onClose` sees a pane's last view as it goes.
 */
export function definePaneViews<V extends object>(initial: V, onClose?: (paneId: string, view: V) => void): PaneViews<V> {
    const useViews = create<{ views: Record<string, V> }>()(() => ({ views: {} }));

    onPaneClosed((paneId) => {
        const { views } = useViews.getState();
        if (!(paneId in views)) return;
        onClose?.(paneId, views[paneId]);
        const rest = { ...views };
        delete rest[paneId];
        useViews.setState({ views: rest });
    });

    return {
        use: (paneId) => useViews((state) => state.views[paneId] ?? initial),
        get: (paneId) => useViews.getState().views[paneId] ?? initial,
        update: (paneId, change) =>
            useViews.setState((state) => ({ views: { ...state.views, [paneId]: { ...(state.views[paneId] ?? initial), ...change } } })),
    };
}
