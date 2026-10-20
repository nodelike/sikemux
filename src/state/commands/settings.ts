import { agentApi, type AgentAccountBook } from "../../api/agents";
import { coreCall } from "../../api/core";
import type { ProjectSettings } from "../../api/generated/wire";
import { checkForUpdateNow } from "../../api/updater";
import { adoptAgentAccounts } from "../persist";
import { reportError } from "../toast";
import { invalidate } from "../resources";
import { projectRootsScanR } from "../resources.defs";
import { getState, setState } from "../store";
import type { AccountAgentType } from "../../agents/catalog";

export const setCloudBrowser = (v: string): void => setState({ cloudBrowser: v.trim() });
export const setCloudBrowserShortcut = (v: string): void => setState({ cloudBrowserShortcut: v.trim() });

export const setRestoreAgentTabs = (value: boolean): void => setState({ restoreAgentTabs: value });
export const setAgentNotifications = (value: boolean): void => setState({ agentNotifications: value });
export const setVoiceDictation = (value: boolean): void => setState({ voiceDictation: value });
export const setNotch = (patch: Partial<import("../../notch/notchSettings").NotchSettings>): void =>
    setState({ notch: { ...getState().notch, ...patch } });
export const setIosSimulator = (value: boolean): void => setState({ iosSimulator: value });
export const setSimulatorFrame = (value: boolean): void => setState({ simulatorFrame: value });
export const setPaneShader = (value: boolean): void => setState({ paneShader: value });
export const setBuddies = (value: boolean): void => setState({ buddies: value });
export const setBuddyStyle = (pack: string, shape: string): void => setState({ buddyPack: pack, buddyShape: shape });
export const setBuddyMotion = (value: boolean): void => setState({ buddyMotion: value });
export const setPaneImage = (path: string | null): void => setState({ paneImage: path });
export const clampPaneStrength = (value: number): number => Math.min(2, Math.max(0, Math.round(value * 20) / 20));
export const setPaneStrength = (value: number): void => setState({ paneStrength: clampPaneStrength(value) });
export const setUiTextScale = (value: number): void => setState({ uiTextScale: [1, 1.1, 1.25].includes(value) ? value : 1 });

export const setRailDensity = (value: import("../types").RailDensity): void => setState({ railDensity: value });
export const setMidTurnSend = (value: import("../types").MidTurnSend): void => setState({ midTurnSend: value });
export const setTabRailAllAgents = (value: boolean): void => setState({ tabRailAllAgents: value });
export const setTabRailScope = (value: import("../types").TabRailScope): void => setState({ tabRailScope: value });
export const setTabRailPage = (value: import("../types").TabRailPage): void => setState({ tabRailPage: value });
export const setDefaultAgentPermissionMode = (value: import("../types").AgentPermissionMode): void =>
    setState({ defaultAgentPermissionMode: value === "bypass" ? "bypass" : "workspace-write" });
/* The person's accounts are the core's; each change goes there and the accounts it answers with come back. */
const keepAccounts = (change: Promise<AgentAccountBook>, what: string): Promise<void> => change.then(adoptAgentAccounts).catch(reportError(what));
export const setAccountAutoSwitch = (provider: AccountAgentType, on: boolean): Promise<void> =>
    keepAccounts(agentApi.setAutoSwitch(provider, on), "Switch accounts at limits");
export const saveProviderProfile = (profile: import("../types").ProviderProfile): Promise<void> =>
    keepAccounts(agentApi.saveAccount(profile), "Save account");
export function deleteProviderProfile(id: string): Promise<void> {
    if (id.startsWith("builtin-")) return Promise.resolve();
    return keepAccounts(agentApi.removeAccount(id), "Remove account");
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

/** Changes what new chats start with in the project at `cwd`; an undefined field goes back to the app's default. */
export function setProjectSettings(cwd: string, change: Partial<ProjectSettings>): void {
    const merged = { ...getState().projectSettings[cwd], ...change };
    const accounts = Object.fromEntries(Object.entries(merged.accounts ?? {}).filter(([, account]) => account));
    const settings: ProjectSettings = {
        ...(merged.agent ? { agent: merged.agent } : {}),
        ...(merged.permissionMode ? { permissionMode: merged.permissionMode } : {}),
        ...(Object.keys(accounts).length ? { accounts } : {}),
    };
    setState((s) => {
        const { [cwd]: _replaced, ...others } = s.projectSettings;
        return { projectSettings: Object.keys(settings).length ? { ...others, [cwd]: settings } : others };
    });
    void coreCall("setProjectSettings", { path: cwd, settings }).catch(reportError("Save project settings"));
}

export const setAgentWorktreeDefault = (project: string, on: boolean): void =>
    setState((s) => ({ agentWorktreeDefaults: { ...s.agentWorktreeDefaults, [project]: on } }));

/** Null clears it, so the project's Git views go back to asking. */
export function setDefaultGitRepo(project: string, repo: string | null): void {
    setState((s) => {
        const { [project]: _cleared, ...rest } = s.defaultGitRepos;
        return { defaultGitRepos: repo ? { ...rest, [project]: repo } : rest };
    });
}

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

const samePin = (pin: import("../types").PinnedChat, agent: string, id: string) => pin.agent === agent && pin.id === id;

/** Keeps the chat at the top of the tab rail, open or closed. */
export const pinChat = (chat: import("../types").PinnedChat): void =>
    setState((state) => (state.pinnedChats.some((pin) => samePin(pin, chat.agent, chat.id)) ? {} : { pinnedChats: [...state.pinnedChats, chat] }));

export const unpinChat = (agent: string, id: string): void =>
    setState((state) => ({ pinnedChats: state.pinnedChats.filter((pin) => !samePin(pin, agent, id)) }));

/** A pinned chat keeps the name it was last given. */
export const retitlePinnedChat = (agent: string, id: string, title: string): void =>
    setState((state) =>
        state.pinnedChats.some((pin) => samePin(pin, agent, id))
            ? { pinnedChats: state.pinnedChats.map((pin) => (samePin(pin, agent, id) ? { ...pin, title } : pin)) }
            : {},
    );
