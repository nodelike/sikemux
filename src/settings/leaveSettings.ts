import { useEffect } from "react";
import * as cmd from "../state/commands";
import { useStore, type StoreState } from "../state/store";

/** Marks a surface whose buttons take you somewhere, so pressing one while settings is open closes it. */
export const LEAVES_SETTINGS = { "data-leaves-settings": "" } as const;

const NAVIGATES = "button, a, [role='button'], [role='tab'], [role='treeitem']";

function placeInFront(state: StoreState): string {
    const session = state.sessions[state.activeSessionId];
    const win = session ? state.windows[session.activeWindowId] : undefined;
    return [state.activeSessionId, session?.activeWindowId, win?.activePaneId, state.zoomedPaneId].join("\u0000");
}

/** Settings stands in for the workspace, so going anywhere in the workspace leaves it. */
export function useLeaveSettingsOnNavigation(): void {
    useLeaveOnNavigation((state) => state.settingsOpen, cmd.closeSettings);
}

/** The same for any view that stands in for the workspace while `isOpen` says it is open. */
export function useLeaveOnNavigation(isOpen: (state: StoreState) => boolean, close: () => void): void {
    useEffect(() => {
        const unsubscribe = useStore.subscribe((state, previous) => {
            if (isOpen(state) && placeInFront(state) !== placeInFront(previous)) close();
        });
        const onClick = (event: MouseEvent) => {
            const target = event.target instanceof Element ? event.target : null;
            if (target?.closest(NAVIGATES)?.closest("[data-leaves-settings]")) close();
        };
        document.addEventListener("click", onClick, true);
        return () => {
            unsubscribe();
            document.removeEventListener("click", onClick, true);
        };
    }, [isOpen, close]);
}
