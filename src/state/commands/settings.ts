import { checkForUpdateNow } from "../../api/updater";
import { invalidate } from "../resources";
import { projectRootsScanR } from "../resources.defs";
import { getState, setState } from "../store";
import type { AgentType } from "../types";

export const setCloudBrowser = (v: string): void => setState({ cloudBrowser: v.trim() });
export const setCloudBrowserShortcut = (v: string): void => setState({ cloudBrowserShortcut: v.trim() });

export const setRestoreAgentTabs = (value: boolean): void => setState({ restoreAgentTabs: value });
export const setAgentNotifications = (value: boolean): void => setState({ agentNotifications: value });
export const setVoiceDictation = (value: boolean): void => setState({ voiceDictation: value });
export const setPaneShader = (value: boolean): void => setState({ paneShader: value });
export const setPaneImage = (path: string | null): void => setState({ paneImage: path });
export const setUiTextScale = (value: number): void => setState({ uiTextScale: [1, 1.1, 1.25].includes(value) ? value : 1 });

export const setRailDensity = (value: import("../types").RailDensity): void => setState({ railDensity: value });
export const setAgentRailAllAgents = (value: boolean): void => setState({ agentRailAllAgents: value });
export const setAgentRailScope = (value: import("../types").AgentRailScope): void => setState({ agentRailScope: value });
export const setDefaultAgentPermissionMode = (value: import("../types").AgentPermissionMode): void =>
    setState({ defaultAgentPermissionMode: value === "bypass" ? "bypass" : "workspace-write" });
export function selectProviderProfile(type: AgentType, profileId: string): void {
    setState((state) => ({ selectedProviderProfileIds: { ...state.selectedProviderProfileIds, [type]: profileId } }));
}
export function saveProviderProfile(profile: import("../types").ProviderProfile): void {
    setState((state) => {
        const selectedProviderProfileIds = { ...state.selectedProviderProfileIds };
        for (const [type, selected] of Object.entries(selectedProviderProfileIds)) {
            if (selected === profile.id && type !== profile.provider) delete selectedProviderProfileIds[type as AgentType];
        }
        return {
            providerProfiles: [...state.providerProfiles.filter((item) => item.id !== profile.id), profile],
            selectedProviderProfileIds,
        };
    });
}
export function deleteProviderProfile(id: string): void {
    if (id.startsWith("builtin-")) return;
    setState((state) => {
        const selectedProviderProfileIds = { ...state.selectedProviderProfileIds };
        for (const [type, selected] of Object.entries(selectedProviderProfileIds)) {
            if (selected === id) delete selectedProviderProfileIds[type as AgentType];
        }
        return { providerProfiles: state.providerProfiles.filter((profile) => profile.id !== id), selectedProviderProfileIds };
    });
}
export const checkForUpdates = (): Promise<void> => checkForUpdateNow();

export const setUpdateChannel = (value: "stable" | "nightly"): void => {
    if (getState().updateChannel === value) return;
    // The other channel's result says nothing about this one, and waiting out
    // the 30-minute poll to learn what the new channel offers reads as broken.
    setState({ updateChannel: value, pendingUpdate: null, lastUpdateCheck: null });
    void checkForUpdateNow();
};

export const setShareUsageData = (value: boolean): void => setState({ shareUsageData: value });

export const setLanguageServerTrust = (project: string, allowed: boolean): void =>
    setState((s) => ({ languageServerTrust: { ...s.languageServerTrust, [project]: allowed } }));

export const setAgentWorktreeDefault = (project: string, on: boolean): void =>
    setState((s) => ({ agentWorktreeDefaults: { ...s.agentWorktreeDefaults, [project]: on } }));

export function setKeybinding(id: import("../../commands/keybindings").KeybindingActionId, binding: string | null): void {
    setState((s) => ({ keybindingOverrides: { ...s.keybindingOverrides, [id]: binding } }));
}

export function resetKeybinding(id: import("../../commands/keybindings").KeybindingActionId): void {
    setState((s) => {
        const keybindingOverrides = { ...s.keybindingOverrides };
        delete keybindingOverrides[id];
        return { keybindingOverrides };
    });
}

export const resetAllKeybindings = (): void => setState({ keybindingOverrides: {} });

export function addProjectRoot(path: string, depth = 1, selfIndex = false): void {
    const boundedDepth = Math.max(0, Math.min(8, Math.round(Number.isFinite(depth) ? depth : 1)));
    setState((s) =>
        s.projectRoots.some((r) => r.path === path) ? {} : { projectRoots: [...s.projectRoots, { path, depth: boundedDepth, selfIndex }] },
    );
    invalidate((kind) => kind === projectRootsScanR.kind);
}

/** Index the folder itself as a project, on top of whatever its depth finds. */
export function setProjectRootSelfIndex(path: string, selfIndex: boolean): void {
    setState((s) => ({
        projectRoots: s.projectRoots.map((r) => (r.path === path ? { ...r, selfIndex } : r)),
    }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export function removeProjectRoot(path: string): void {
    setState((s) => ({
        projectRoots: s.projectRoots.filter((r) => r.path !== path),
    }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export function setProjectRootDepth(path: string, depth: number): void {
    const d = Math.max(0, Math.min(8, Math.round(Number.isFinite(depth) ? depth : 1)));
    setState((s) => ({
        projectRoots: s.projectRoots.map((r) => (r.path === path ? { ...r, depth: d } : r)),
    }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}
