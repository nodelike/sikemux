import { clampRailWidth, type RailEdge } from "../../lib/railWidths";
import { invalidate } from "../resources";
import { getState, mutate, setState } from "../store";
import type { SettingsPageId } from "../../settings/settingsIndex";
import type { PickerMode } from "../types";

export const setHome = (home: string): void => setState({ home });
export const setLastSessionId = (id: string): void => setState({ lastSessionId: id });
export const setTerminalTitle = (paneId: string, title: string): void =>
    setState((s) => ({ terminalTitles: { ...s.terminalTitles, [paneId]: title } }));
export const openPicker = (mode: PickerMode = "all"): void => setState({ pickerOpen: true, pickerMode: mode });
export const closePicker = (): void => setState({ pickerOpen: false });
// The agent picker is project-scoped and opens over the agent view.
export const openAgentPalette = (): void => {
    invalidate((kind) => kind === "agents.catalog" || kind === "agents.models" || kind === "agents.usage");
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
export const openSettings = (page?: SettingsPageId): void => setState(page ? { settingsOpen: true, settingsPage: page } : { settingsOpen: true });
export const setSettingsPage = (page: SettingsPageId): void => setState({ settingsPage: page });
export const closeSettings = (): void => setState({ settingsOpen: false });
export const toggleSettings = (): void => setState((s) => ({ settingsOpen: !s.settingsOpen }));

// Focus mode hides both rails, so asking for one back has to leave focus mode.
export const toggleSideRail = (): void => setState((s) => (s.zenMode ? { zenMode: false, sideRailOpen: true } : { sideRailOpen: !s.sideRailOpen }));
export const toggleAgentRail = (): void =>
    setState((s) => (s.zenMode ? { zenMode: false, agentRailOpen: true } : { agentRailOpen: !s.agentRailOpen }));
export const setRailWidth = (edge: RailEdge, px: number): void =>
    setState(edge === "start" ? { sideRailWidth: clampRailWidth(edge, px) } : { agentRailWidth: clampRailWidth(edge, px) });
export const toggleZen = (): void => setState((s) => ({ zenMode: !s.zenMode, sideRailOpen: s.zenMode, agentRailOpen: s.zenMode }));
