import { agentApi, type AgentAccountBook } from "../../api/agents";
import { coreCall } from "../../api/core";
import type { FaceStyle } from "../../api/generated/wire";
import { checkForUpdateNow } from "../../api/updater";
import { adoptAgentAccounts } from "../persist";
import { reportError } from "../toast";
import { invalidate } from "../resources";
import { projectRootsScanR } from "../resources.defs";
import { preferencesOf, projectRootsOf, setPreference, updateChannelOf } from "../preferences";
import { MAX_PROJECT_DEPTH, type NotchSettings, type ProjectRoot } from "../../api/generated/wire";
import { getState, setState } from "../store";
import type { AgentType } from "../../agents/catalog";

export const setCloudBrowser = (v: string): void => setPreference("cloudBrowser", v.trim());
export const setCloudBrowserShortcut = (v: string): void => setPreference("cloudBrowserShortcut", v.trim());

export const setRestoreAgentTabs = (value: boolean): void => setPreference("restoreAgentTabs", value);
export const setAgentNotifications = (value: boolean): void => setPreference("agentNotifications", value);
export const setVoiceDictation = (value: boolean): void => setPreference("voiceDictation", value);
export function setNotch(patch: Partial<NotchSettings>): void {
    const notch = getState().preferences?.notch;
    if (notch) setPreference("notch", { ...notch, ...patch });
}
export const setIosSimulator = (value: boolean): void => setPreference("iosSimulator", value);
export const setSimulatorFrame = (value: boolean): void => setPreference("simulatorFrame", value);
export const setPaneShader = (value: boolean): void => setPreference("paneShader", value);
/** Changes how agents' faces look for every client, showing the change here at once. */
function changeFaceStyle(change: Partial<FaceStyle>): void {
    setState({ faceStyle: { ...getState().faceStyle, ...change } });
    coreCall<FaceStyle>("setFaceStyle", change)
        .then((faceStyle) => setState({ faceStyle }))
        .catch(reportError("Change buddies"));
}
export const setBuddies = (on: boolean): void => changeFaceStyle({ on });
export const setBuddyStyle = (pack: string, shape: string): void => changeFaceStyle({ pack, shape });
export const setBuddyMotion = (motion: boolean): void => changeFaceStyle({ motion });
export const setPaneImage = (path: string | null): void => setPreference("paneImage", path);
export const clampPaneStrength = (value: number): number => Math.min(2, Math.max(0, Math.round(value * 20) / 20));
export const setPaneStrength = (value: number): void => setPreference("paneStrength", clampPaneStrength(value));
export const setUiTextScale = (value: number): void => setPreference("uiTextScale", value);

export const setRailDensity = (value: import("../types").RailDensity): void => setPreference("railDensity", value);
export const setTabRailAllAgents = (value: boolean): void => setState({ tabRailAllAgents: value });
export const setTabRailScope = (value: import("../types").TabRailScope): void => setState({ tabRailScope: value });
export const setTabRailPage = (value: import("../types").TabRailPage): void => setState({ tabRailPage: value });
export const setDefaultAgentPermissionMode = (value: import("../types").AgentPermissionMode): void =>
    setState({ defaultAgentPermissionMode: value === "bypass" ? "bypass" : "workspace-write" });
/* The person's accounts are the core's; each change goes there and the accounts it answers with come back. */
const keepAccounts = (change: Promise<AgentAccountBook>, what: string): Promise<void> => change.then(adoptAgentAccounts).catch(reportError(what));
export const setAccountAutoSwitch = (provider: AgentType, on: boolean): Promise<void> =>
    keepAccounts(agentApi.setAutoSwitch(provider, on), "Switch accounts at limits");
export const saveProviderProfile = (profile: import("../types").ProviderProfile): Promise<void> =>
    keepAccounts(agentApi.saveAccount(profile), "Save account");
export function deleteProviderProfile(id: string): Promise<void> {
    if (id.startsWith("builtin-")) return Promise.resolve();
    return keepAccounts(agentApi.removeAccount(id), "Remove account");
}
export const checkForUpdates = (): Promise<void> => checkForUpdateNow();

export const setUpdateChannel = (value: "stable" | "nightly"): void => {
    if (updateChannelOf(getState()) === value) return;
    // The other channel's result says nothing about this one; the core checks
    // the new channel as soon as the preference reaches it.
    setPreference("updateChannel", value);
    setState({ pendingUpdate: null, lastUpdateCheck: null });
};

export const setShareUsageData = (value: boolean): void => setPreference("shareUsageData", value);

export const setLanguageServerTrust = (project: string, allowed: boolean): void =>
    setPreference("languageServerTrust", { ...getState().preferences?.languageServerTrust, [project]: allowed });

export const setAgentWorktreeDefault = (project: string, on: boolean): void =>
    setPreference("agentWorktreeDefaults", { ...getState().preferences?.agentWorktreeDefaults, [project]: on });

/** Null clears it, so the project's Git views go back to asking. */
export function setDefaultGitRepo(project: string, repo: string | null): void {
    const { [project]: _cleared, ...rest } = getState().preferences?.defaultGitRepos ?? {};
    setPreference("defaultGitRepos", repo ? { ...rest, [project]: repo } : rest);
}

export function setKeybinding(id: import("../../commands/keybindings").KeybindingActionId, binding: string | null): void {
    setPreference("keybindingOverrides", { ...preferencesOf(getState()).keybindingOverrides, [id]: binding });
}

export function resetKeybinding(id: import("../../commands/keybindings").KeybindingActionId): void {
    const { [id]: _reset, ...keybindingOverrides } = preferencesOf(getState()).keybindingOverrides;
    setPreference("keybindingOverrides", keybindingOverrides);
}

export const resetAllKeybindings = (): void => setPreference("keybindingOverrides", {});

const boundedDepth = (depth: number): number => Math.max(0, Math.min(MAX_PROJECT_DEPTH, Math.round(Number.isFinite(depth) ? depth : 1)));

function setProjectRoots(change: (roots: ProjectRoot[]) => ProjectRoot[]): void {
    setPreference("projectRoots", change(projectRootsOf(getState())));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export function addProjectRoot(path: string, depth = 1, selfIndex = false): void {
    setProjectRoots((roots) => (roots.some((r) => r.path === path) ? roots : [...roots, { path, depth: boundedDepth(depth), selfIndex }]));
}

/** Index the folder itself as a project, on top of whatever its depth finds. */
export function setProjectRootSelfIndex(path: string, selfIndex: boolean): void {
    setProjectRoots((roots) => roots.map((r) => (r.path === path ? { ...r, selfIndex } : r)));
}

export function removeProjectRoot(path: string): void {
    setProjectRoots((roots) => roots.filter((r) => r.path !== path));
}

export function setProjectRootDepth(path: string, depth: number): void {
    setProjectRoots((roots) => roots.map((r) => (r.path === path ? { ...r, depth: boundedDepth(depth) } : r)));
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
