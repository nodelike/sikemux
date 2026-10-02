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
    useEffect(() => {
        const unsubscribe = useStore.subscribe((state, previous) => {
            if (state.settingsOpen && placeInFront(state) !== placeInFront(previous)) cmd.closeSettings();
        });
        const onClick = (event: MouseEvent) => {
            const target = event.target instanceof Element ? event.target : null;
            if (target?.closest(NAVIGATES)?.closest("[data-leaves-settings]")) cmd.closeSettings();
        };
        document.addEventListener("click", onClick, true);
        return () => {
            unsubscribe();
            document.removeEventListener("click", onClick, true);
        };
    }, []);
}
