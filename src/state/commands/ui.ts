import { clampRailWidth, type RailEdge } from "../../lib/railWidths";
import { invalidate } from "../resources";
import { selectFocusMode } from "../selectors";
import { getState, mutate, setState } from "../store";
import type { SettingsPageId } from "../../settings/settingsIndex";
import type { ProjectSettingsPageId } from "../../settings/projectSettingsPages";
import type { PickerMode } from "../types";

export const setHome = (home: string): void => setState({ home });
export const setLastSessionId = (id: string): void => setState({ lastSessionId: id });
export const openPicker = (mode: PickerMode = "all"): void => setState({ pickerOpen: true, pickerMode: mode, pickerMovingAgentId: null });
export const openMoveToProject = (agentId: string): void => setState({ pickerOpen: true, pickerMode: "projects", pickerMovingAgentId: agentId });
export const closePicker = (): void => setState({ pickerOpen: false });
// The agent picker is project-scoped and opens over the agent view.
export const openAgentPalette = (): void => {
    invalidate((kind) => kind === "agents.catalog" || kind === "agents.usage");
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (session?.kind !== "project") return;
        d.agentPaletteOpen = true;
        d.zoomedPaneId = null;
    });
};
export const closeAgentPalette = (): void => {
    const state = getState();
    const session = state.sessions[state.activeSessionId];
    // A project with nothing open keeps the picker, or it would show a blank stage.
    if (session?.kind === "project" && (state.windowsBySession[session.id] ?? []).length === 0) return;
    setState({ agentPaletteOpen: false });
};
export const forceCloseAgentPalette = (): void => setState({ agentPaletteOpen: false });
export const openCommandPalette = (): void => setState({ commandPaletteOpen: true });
export const closeCommandPalette = (): void => setState({ commandPaletteOpen: false });
export const toggleCommandPalette = (): void => setState((s) => ({ commandPaletteOpen: !s.commandPaletteOpen }));
export const openThemePalette = (): void => setState({ themePaletteOpen: true, commandPaletteOpen: false });
export const closeThemePalette = (): void => setState({ themePaletteOpen: false });
export const openOnboarding = (): void => setState({ onboardingOpen: true, diagnosticsOpen: false, whatsNewOpen: false });
export const closeOnboarding = (complete = true): void => setState({ onboardingOpen: false, ...(complete ? { onboardingComplete: true } : {}) });
export const openDiagnostics = (): void => setState({ diagnosticsOpen: true, onboardingOpen: false, whatsNewOpen: false });
export const closeDiagnostics = (): void => setState({ diagnosticsOpen: false });
export const openWhatsNew = (): void => setState({ whatsNewOpen: true, onboardingOpen: false, diagnosticsOpen: false });
export const closeWhatsNew = (): void =>
    setState((s) => ({ whatsNewOpen: false, lastSeenVersion: s.lastReleaseNotes?.version ?? s.lastSeenVersion }));
export const openNewTabPalette = (): void =>
    setState({ newTabPaletteOpen: true, filePaletteOpen: false, agentPaletteOpen: false, pickerOpen: false });
export const closeNewTabPalette = (): void => setState({ newTabPaletteOpen: false });

export const openFilePalette = (): void => setState({ filePaletteOpen: true });
export const closeFilePalette = (): void => setState({ filePaletteOpen: false });
export const openSettings = (page?: SettingsPageId, target?: string): void =>
    setState(page ? { settingsOpen: true, settingsPage: page, settingsTarget: target ?? null } : { settingsOpen: true });
export const clearSettingsTarget = (): void => setState({ settingsTarget: null });
export const setSettingsPage = (page: SettingsPageId): void => setState({ settingsPage: page });
export const dismissSignInCard = (): void => setState({ signInCardDismissed: true });
/** Turns `project`'s settings tab to `page`. */
export const setProjectSettingsPage = (project: string, page: ProjectSettingsPageId): void =>
    setState((s) => ({ projectSettingsPages: { ...s.projectSettingsPages, [project]: page } }));
export const closeSettings = (): void => setState({ settingsOpen: false });
export const toggleSettings = (): void => setState((s) => ({ settingsOpen: !s.settingsOpen }));

export const toggleSideRail = (): void => setState((s) => ({ sideRailOpen: !s.sideRailOpen }));
export const toggleTabRail = (): void => setState((s) => ({ tabRailOpen: !s.tabRailOpen }));
export const setRailWidth = (edge: RailEdge, px: number): void =>
    setState(edge === "start" ? { sideRailWidth: clampRailWidth(edge, px) } : { tabRailWidth: clampRailWidth(edge, px) });
export const toggleZen = (): void =>
    setState((s) => {
        const showRails = selectFocusMode(s);
        return { sideRailOpen: showRails, tabRailOpen: showRails };
    });
