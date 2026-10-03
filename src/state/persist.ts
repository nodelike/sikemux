import { normaliseNotchSettings } from "../notch/notchSettings";
import { invokeCommand as invoke } from "../api/invoke";
import { fixedSessionName } from "./sessionNames";
import { sshStartup } from "../terminal/sshStartup";
import { clampTerminalFontSize } from "../terminal/fontSize";
import { clampChatTextScale } from "../chat/textScale";
import { clampEditorTextScale } from "../editor/textScale";
import { isTheme } from "../themes";
import { parseReleaseCredits } from "../api/releases";
import { normaliseKeybindingOverrides } from "../commands/keybindings";
import type { CommandContext, CustomCommand, CustomCommandPlacement } from "../commands/registry";
import { registerCustomThemes } from "../themes/bus";
import { normalizePermissionMode } from "../agents/agentLaunch";
import { clampRailWidth } from "../lib/railWidths";
import { mergePinnedIntoRoots, normaliseProjectRoots, pruneOnDemandWindows } from "./commands";
import { agentPaneId } from "./selectors";
import { collectPanes, removePane } from "./layout";
import { BROWSER_ACTIVE, deskEditorId, deskView, fileKey } from "./desks";
import { agentDirectCommand, agentStartup } from "./commands";
import { agentWindow } from "./agentWindow";
import { getState, setState, useStore, type StoreState } from "./store";
import { errMessage, notify } from "./toast";
import { isCoreSessionId, isSessionKind, validatePersistedLayout } from "./persistValidation";
import { isPluginId, isPluginKind } from "../plugins/kinds";
import { firstGrapheme, spaceName } from "./projectSpaces";
import { createWorkbenchItemRef, workbenchItemRegistry, workbenchItemRefFromPane, type BuiltinWorkbenchItemState } from "../workbench/registry";
import type {
    Agent,
    AgentPermissionMode,
    AgentWorktree,
    AgentProvider,
    AgentType,
    CorePaneKind,
    Desk,
    DeskView,
    EditorPaneView,
    LayoutNode,
    PersistedAgent,
    PersistedPrefs,
    PersistedSession,
    PersistedSnapshot,
    ProjectSpace,
    ProviderProfile,
    ProviderProfileSelection,
    RecentEntry,
    Session,
    Window,
    WindowRole,
} from "./types";

function deriveRole(w: Window): WindowRole {
    if (WINDOW_ROLES.has(w.role) || isPluginKind(w.role)) return w.role;
    if (w.name === "files") return "files";
    if (w.name === "git") return "git";
    if (w.name === "term" || /^\d+$/.test(w.name)) return "term";
    return "named";
}

export const VERSION = 18;
const MIN_SUPPORTED_VERSION = 3;
const ONBOARDING_MIGRATION_VERSION = 6;
const AGENT_PERMISSION_DEFAULT_MIGRATION_VERSION = 9;
const PLUGIN_KIND_MIGRATION_VERSION = 10;
const PLUGIN_SETTINGS_MIGRATION_VERSION = 11;
const ONE_BRUNO_SESSION_MIGRATION_VERSION = 12;
const AWS_PLUGIN_MIGRATION_VERSION = 13;
const BRUNO_PLUGIN_MIGRATION_VERSION = 14;
const RUNDECK_GROUPS_MIGRATION_VERSION = 15;
const DESK_MIGRATION_VERSION = 16;
const GITHUB_IN_GIT_PANE_MIGRATION_VERSION = 17;
const CORE_SESSION_MIGRATION_VERSION = 18;
const RETRY_MS = 1500;
let lastSaved = "";
let activeSnapshot: string | null = null;
let pendingSnapshot: string | null = null;
let saveLoop: Promise<boolean> | null = null;
let retryTimer: number | undefined;
let persistTimer: number | undefined;
let persistenceReady = false;

const PERSISTED_KEYS = [
    "sessions",
    "windows",
    "agents",
    "sessionOrder",
    "windowsBySession",
    "activeSessionId",
    "recent",
    "editorViews",
    "deskPanes",
    "browserStrips",
    "deskRestores",
    "projectRoots",
    "themeId",
    "customThemes",
    "uiTextScale",
    "paneShader",
    "paneImage",
    "terminalFontSize",
    "chatTextScale",
    "editorTextScale",
    "windowOpacity",
    "windowBlur",
    "cloudBrowser",
    "cloudBrowserShortcut",
    "keybindingOverrides",
    "sideRailOpen",
    "agentRailOpen",
    "sideRailWidth",
    "agentRailWidth",
    "pluginSettings",
    "disabledPlugins",
    "restoreAgentTabs",
    "spaces",
    "projectSpaces",
    "activeSpaceId",
    "agentNotifications",
    "notch",
    "voiceDictation",
    "iosSimulator",
    "notificationsIntroduced",
    "keptRunningNoticeShown",
    "railDensity",
    "agentRailAllAgents",
    "agentRailScope",
    "onboardingComplete",
    "lastSeenVersion",
    "customCommands",
    "updateChannel",
    "shareUsageData",
    "lastReleaseNotes",
    "recentCommandKeys",
    "providerProfiles",
    "selectedProviderProfileIds",
    "defaultAgentPermissionMode",
    "lastAgentType",
    "languageServerTrust",
    "agentWorktreeDefaults",
] as const satisfies readonly (keyof StoreState)[];
type PersistedKey = (typeof PERSISTED_KEYS)[number];
type SliceShot = { [K in PersistedKey]: StoreState[K] };
let lastSlices: SliceShot | null = null;

function takeSlices(s: StoreState): SliceShot {
    const out = {} as SliceShot;
    for (const k of PERSISTED_KEYS) (out as Record<string, unknown>)[k] = s[k];
    return out;
}

function slicesEqual(a: SliceShot, b: SliceShot): boolean {
    for (const k of PERSISTED_KEYS) if (a[k] !== b[k]) return false;
    return true;
}

function packPrefs(s: StoreState): PersistedPrefs {
    const providerProfiles = normaliseProviderProfiles(s.providerProfiles, []);
    return {
        projectRoots: s.projectRoots,
        themeId: s.themeId,
        customThemes: s.customThemes,
        uiTextScale: s.uiTextScale,
        paneShader: s.paneShader,
        paneImage: s.paneImage,
        terminalFontSize: s.terminalFontSize,
        chatTextScale: s.chatTextScale,
        editorTextScale: s.editorTextScale,
        windowOpacity: s.windowOpacity,
        windowBlur: s.windowBlur,
        cloudBrowser: s.cloudBrowser,
        cloudBrowserShortcut: s.cloudBrowserShortcut,
        keybindingOverrides: s.keybindingOverrides,
        sideRailOpen: s.sideRailOpen,
        agentRailOpen: s.agentRailOpen,
        sideRailWidth: s.sideRailWidth,
        agentRailWidth: s.agentRailWidth,
        pluginSettings: s.pluginSettings,
        disabledPlugins: [...s.disabledPlugins],
        restoreAgentTabs: s.restoreAgentTabs,
        spaces: [...s.spaces],
        projectSpaces: s.projectSpaces,
        activeSpaceId: s.activeSpaceId,
        agentNotifications: s.agentNotifications,
        notch: s.notch,
        voiceDictation: s.voiceDictation,
        iosSimulator: s.iosSimulator,
        notificationsIntroduced: s.notificationsIntroduced,
        keptRunningNoticeShown: s.keptRunningNoticeShown,
        railDensity: s.railDensity,
        agentRailAllAgents: s.agentRailAllAgents,
        agentRailScope: s.agentRailScope,
        onboardingComplete: s.onboardingComplete,
        lastSeenVersion: s.lastSeenVersion,
        customCommands: s.customCommands,
        updateChannel: s.updateChannel,
        shareUsageData: s.shareUsageData,
        lastReleaseNotes: s.lastReleaseNotes,
        recentCommandKeys: s.recentCommandKeys,
        providerProfiles,
        selectedProviderProfileIds: normaliseProviderProfileSelection(s.selectedProviderProfileIds, providerProfiles, {}),
        defaultAgentPermissionMode: s.defaultAgentPermissionMode === "bypass" ? "bypass" : "workspace-write",
        lastAgentType: s.lastAgentType,
        languageServerTrust: s.languageServerTrust,
        agentWorktreeDefaults: s.agentWorktreeDefaults,
    };
}

const WINDOW_ROLES = new Set<WindowRole>(["term", "files", "git", "diff", "search", "ssh-config", "named", "agent"]);

function normaliseSpaces(value: unknown): ProjectSpace[] {
    if (!Array.isArray(value)) return [];
    const spaces: ProjectSpace[] = [];
    for (const row of value) {
        if (!isRecord(row) || typeof row.id !== "string" || !row.id || typeof row.name !== "string") continue;
        const name = spaceName(row.name);
        if (!name || spaces.some((space) => space.id === row.id)) continue;
        spaces.push({ id: row.id, name, icon: typeof row.icon === "string" ? firstGrapheme(row.icon) : "" });
    }
    return spaces;
}

function normaliseProjectSpaces(value: unknown, spaces: readonly ProjectSpace[]): Record<string, string> {
    if (!isRecord(value)) return {};
    const known = new Set(spaces.map((space) => space.id));
    const projectSpaces: Record<string, string> = {};
    for (const [cwd, id] of Object.entries(value)) if (typeof id === "string" && known.has(id)) projectSpaces[cwd] = id;
    return projectSpaces;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return !!value && typeof value === "object" && !Array.isArray(value);
}

const COMMAND_PLACEMENTS = new Set<CustomCommandPlacement>(["background", "terminal", "split", "popup", "replace"]);

function normaliseCustomCommands(value: unknown): CustomCommand[] {
    if (!Array.isArray(value)) return [];
    const seen = new Set<string>();
    const commands: CustomCommand[] = [];
    for (const row of value) {
        if (!isRecord(row) || typeof row.id !== "string" || !row.id || seen.has(row.id)) continue;
        if (typeof row.title !== "string" || !row.title.trim() || typeof row.command !== "string" || !row.command.trim()) continue;
        if (!COMMAND_PLACEMENTS.has(row.placement as CustomCommandPlacement)) continue;
        const contexts = Array.isArray(row.contexts) ? row.contexts.filter((v): v is CommandContext => isSessionKind(v)) : [];
        seen.add(row.id);
        commands.push({
            id: row.id.slice(0, 100),
            title: row.title.trim().slice(0, 120),
            detail: typeof row.detail === "string" ? row.detail.trim().slice(0, 240) : "",
            command: row.command.slice(0, 8_000),
            contexts,
            placement: row.placement as CustomCommandPlacement,
        });
        if (commands.length >= 100) break;
    }
    return commands;
}

function isLayout(value: unknown): value is Window["root"] {
    return validatePersistedLayout(value).ok;
}

function isWindow(value: unknown): value is Window {
    return (
        isRecord(value) &&
        typeof value.id === "string" &&
        typeof value.name === "string" &&
        typeof value.activePaneId === "string" &&
        isLayout(value.root)
    );
}

function layoutIds(root: Window["root"]): { all: string[]; panes: string[] } {
    const all: string[] = [];
    const panes: string[] = [];
    const walk = (node: Window["root"]): void => {
        all.push(node.id);
        if (node.type === "pane") panes.push(node.id);
        else node.children.forEach(walk);
    };
    walk(root);
    return { all, panes };
}

/** Upgrade saved SSH startups, including the briefly shipped multiline form. */
function upgradeSshStartup(root: Window["root"], alias: string): Window["root"] {
    if (root.type === "pane") {
        const needsUpgrade = root.startup === `ssh ${alias}` || root.startup?.includes("sikemux_ssh_retries");
        return root.kind === "terminal" && needsUpgrade ? { ...root, startup: sshStartup(alias) } : root;
    }
    const children = root.children.map((child) => upgradeSshStartup(child, alias));
    return children.some((child, i) => child !== root.children[i]) ? { ...root, children } : root;
}

function toSession(value: unknown): Session | null {
    if (!isRecord(value)) return null;
    if (
        typeof value.id !== "string" ||
        typeof value.name !== "string" ||
        !isSessionKind(value.kind) ||
        typeof value.cwd !== "string" ||
        typeof value.pinned !== "boolean" ||
        typeof value.activeWindowId !== "string"
    ) {
        return null;
    }
    const session: Session = {
        id: value.id,
        name: fixedSessionName(value.kind as Session["kind"]) ?? value.name,
        kind: value.kind as Session["kind"],
        cwd: value.cwd,
        pinned: value.pinned,
        activeWindowId: value.activeWindowId,
    };
    return session;
}

function isRecent(value: unknown): value is RecentEntry {
    return isRecord(value) && isSessionKind(value.kind) && typeof value.name === "string" && typeof value.cwd === "string";
}

const AGENT_TYPES = new Set<AgentType>(["claude", "codex", "hermes", "pi", "opencode", "omp", "grok"]);
const AGENT_PROVIDERS = new Set<AgentProvider>(["claude", "codex", "gemini"]);
const AGENT_PERMISSION_MODES = new Set<AgentPermissionMode>(["read-only", "workspace-write", "full-access", "bypass"]);

function isAgentPermissionMode(value: unknown): value is AgentPermissionMode {
    return AGENT_PERMISSION_MODES.has(value as AgentPermissionMode);
}

function normaliseProviderProfiles(value: unknown, fallback: ProviderProfile[]): ProviderProfile[] {
    if (!Array.isArray(value)) return fallback.map((profile) => ({ ...profile }));
    const profiles: ProviderProfile[] = [];
    const seen = new Set<string>();
    for (const row of value) {
        if (!isRecord(row) || typeof row.id !== "string" || !row.id.trim() || seen.has(row.id)) continue;
        if (typeof row.name !== "string" || !row.name.trim() || !AGENT_PROVIDERS.has(row.provider as AgentProvider)) continue;
        if (typeof row.accent !== "string" || !/^#[\da-f]{6}$/i.test(row.accent)) continue;
        const id = row.id.trim().slice(0, 100);
        if (seen.has(id)) continue;
        const profile: ProviderProfile = {
            id,
            name: row.name.trim().slice(0, 100),
            provider: row.provider as AgentProvider,
            accent: row.accent.toLowerCase(),
        };
        const executablePath = boundedOptionalString(row.executablePath, 4096);
        const configPath = boundedOptionalString(row.configPath, 4096);
        if (executablePath) profile.executablePath = executablePath;
        if (configPath) profile.configPath = configPath;
        if (Array.isArray(row.environmentKeys)) {
            profile.environmentKeys = [
                ...new Set(
                    row.environmentKeys.filter(
                        (key): key is string => typeof key === "string" && /^[A-Za-z_][A-Za-z0-9_]*$/.test(key) && key.length <= 128,
                    ),
                ),
            ].slice(0, 64);
        }
        seen.add(id);
        profiles.push(profile);
        if (profiles.length >= 50) break;
    }
    return profiles;
}

function normaliseProviderProfileSelection(
    value: unknown,
    profiles: ProviderProfile[],
    fallback: ProviderProfileSelection,
): ProviderProfileSelection {
    if (!isRecord(value)) return { ...fallback };
    const profilesById = new Map(profiles.map((profile) => [profile.id, profile]));
    const selection: ProviderProfileSelection = {};
    for (const type of AGENT_TYPES) {
        const selected = value[type];
        if (typeof selected !== "string") continue;
        const profile = profilesById.get(selected);
        if (profile && (type === "claude" || type === "codex") && profile.provider === type) selection[type] = selected;
    }
    return selection;
}

function boundedOptionalString(value: unknown, max: number): string | undefined {
    return typeof value === "string" && value.trim() && !/[\0\r\n]/.test(value) ? value.slice(0, max) : undefined;
}

function toAgentWorktree(value: unknown): AgentWorktree | undefined {
    if (!isRecord(value)) return undefined;
    const repo = boundedOptionalString(value.repo, 4096);
    const path = boundedOptionalString(value.path, 4096);
    const branch = boundedOptionalString(value.branch, 255);
    const startSha = boundedOptionalString(value.startSha, 64);
    if (!repo || !path || !branch || !startSha) return undefined;
    return { repo, path, branch, base: boundedOptionalString(value.base, 255) ?? null, startSha };
}

const AGENT_EFFORTS = new Set(["off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]);
function toPersistedAgent(value: unknown): PersistedAgent | null {
    if (!isRecord(value) || typeof value.id !== "string" || !value.id || !AGENT_TYPES.has(value.type as AgentType)) return null;
    if (typeof value.title !== "string" || !value.title.trim()) return null;
    const resumeId = typeof value.resumeId === "string" && value.resumeId.trim() ? value.resumeId : undefined;
    const ptyId = isCoreSessionId(value.ptyId) ? value.ptyId : undefined;
    if (!resumeId && ptyId === undefined) return null;
    const agent: PersistedAgent = { id: value.id, type: value.type as AgentType, title: value.title.slice(0, 200) };
    if (resumeId) agent.resumeId = resumeId;
    if (ptyId !== undefined) agent.ptyId = ptyId;
    const requestedPermissionMode = isAgentPermissionMode(value.permissionMode)
        ? value.permissionMode
        : value.skipPermissions === true
          ? "bypass"
          : value.skipPermissions === false
            ? "workspace-write"
            : undefined;
    const permissionMode = requestedPermissionMode ? normalizePermissionMode(value.type as AgentType, requestedPermissionMode) : undefined;
    if (permissionMode) agent.permissionMode = permissionMode;
    if (permissionMode === "bypass") agent.skipPermissions = true;
    const profileId = boundedOptionalString(value.profileId, 100);
    const executablePath = boundedOptionalString(value.executablePath, 4096);
    const cwd = boundedOptionalString(value.cwd, 4096);
    const model = boundedOptionalString(value.model, 200);
    if (profileId) agent.profileId = profileId;
    if (executablePath) agent.executablePath = executablePath;
    if (cwd) agent.cwd = cwd;
    if (model) agent.model = model;
    if (typeof value.effort === "string" && AGENT_EFFORTS.has(value.effort)) agent.effort = value.effort as PersistedAgent["effort"];
    if (value.keepAlive === true) agent.keepAlive = true;
    if (value.renamed === true) agent.renamed = true;
    const worktree = toAgentWorktree(value.worktree);
    if (worktree) agent.worktree = worktree;
    return agent;
}

function persistedSession(sess: Session): PersistedSession {
    return sess;
}

/** Startup commands are rebuilt from the type and resume id on restore, never saved. */
function persistedAgent(agent: Agent): PersistedAgent {
    const permissionMode = agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write");
    return {
        id: agent.id,
        type: agent.type,
        title: agent.title,
        resumeId: agent.resumeId,
        permissionMode,
        ...(permissionMode === "bypass" ? { skipPermissions: true } : {}),
        ...(agent.profileId ? { profileId: agent.profileId } : {}),
        ...(agent.executablePath ? { executablePath: agent.executablePath } : {}),
        ...(agent.cwd ? { cwd: agent.cwd } : {}),
        ...(agent.model ? { model: agent.model } : {}),
        ...(agent.effort ? { effort: agent.effort } : {}),
        ...(agent.keepAlive ? { keepAlive: true } : {}),
        ...(agent.renamed ? { renamed: true } : {}),
        ...(agent.worktree ? { worktree: agent.worktree } : {}),
        ...(agent.ptyId !== undefined ? { ptyId: agent.ptyId } : {}),
    };
}

/**
 * A desk comes back from its saved pages and files, so one with nothing left
 * to open would restore as a blank half and is dropped instead.
 */
function withoutEmptyDesks(window: Window): Window | null {
    const emptyPaneIds = collectPanes(window.root)
        .filter((pane) => pane.kind === "desk" && !deskView(pane.id))
        .map((pane) => pane.id);
    if (emptyPaneIds.length === 0) return window;
    let root: LayoutNode | null = window.root;
    for (const paneId of emptyPaneIds) root = root && removePane(root, paneId);
    if (!root) return null;
    const panes = collectPanes(root);
    const activePaneId = panes.some((pane) => pane.id === window.activePaneId) ? window.activePaneId : panes[0].id;
    return { ...window, root, activePaneId };
}

/**
 * A window worth writing. A task terminal is runtime-only, and an agent with
 * neither a resume id nor a terminal in the core could not be brought back, so
 * neither goes to disk.
 */
function durableWindow(s: StoreState, id: string): Window | null {
    const window = s.windows[id];
    if (!window || window.transient) return null;
    const agent = window.role === "agent" ? s.agents[agentPaneId(window) ?? ""] : null;
    if (window.role === "agent" && !agent?.resumeId && agent?.ptyId === undefined) return null;
    return withoutEmptyDesks(window);
}

/** One malformed item must not cost every other item, or the layout, its save. */
function encodeItemState<Kind extends CorePaneKind>(
    itemStates: NonNullable<PersistedSnapshot["itemStates"]>,
    itemId: string,
    kind: Kind,
    state: BuiltinWorkbenchItemState[Kind],
): void {
    try {
        itemStates[itemId] = workbenchItemRegistry.encodePersisted(createWorkbenchItemRef(itemId, kind), state);
    } catch {
        return;
    }
}

function snapshot(): string {
    const s = getState();
    const sessions = s.sessionOrder
        .map((id) => s.sessions[id])
        .filter(Boolean)
        .map((sess) => {
            const durableWindowIds = (s.windowsBySession[sess.id] ?? []).filter((id) => durableWindow(s, id));
            const activeWindowId = durableWindowIds.includes(sess.activeWindowId) ? sess.activeWindowId : (durableWindowIds[0] ?? "");
            return persistedSession({ ...sess, activeWindowId });
        });
    const windowsBySession: Record<string, Window[]> = {};
    const agents: PersistedAgent[] = [];
    const itemStates: PersistedSnapshot["itemStates"] = {};
    for (const sess of sessions) {
        windowsBySession[sess.id] = (s.windowsBySession[sess.id] ?? []).flatMap((id) => {
            const window = durableWindow(s, id);
            return window ? [window] : [];
        });
        for (const window of windowsBySession[sess.id]) {
            if (window.role === "agent") {
                const agent = s.agents[agentPaneId(window) ?? ""];
                if (agent) agents.push(persistedAgent(agent));
            }
            const pending = [window.root];
            while (pending.length > 0) {
                const node = pending.pop()!;
                if (node.type === "split") {
                    pending.push(...node.children);
                    continue;
                }
                if (node.kind === "editor")
                    encodeItemState(itemStates, node.id, "editor", s.editorViews[node.id] ?? { openTabs: [], activePath: null });
                if (node.kind === "desk") {
                    const view = deskView(node.id);
                    if (view) encodeItemState(itemStates, node.id, "desk", view);
                }
            }
        }
    }
    const snap: PersistedSnapshot = {
        version: VERSION,
        sessions,
        windowsBySession,
        agents,
        sessionOrder: sessions.map((s) => s.id),
        activeSessionId: s.activeSessionId,
        recent: s.recent,
        prefs: packPrefs(s),
        itemStates,
    };
    // Defense in depth: these runtime-only fields must never reach disk, even if
    // a malformed record introduced them outside the typed shapes above.
    return JSON.stringify(snap, (key, value) =>
        key === "secretVars" || key === "drafts" || key === "transient" || key === "externalPty" || key === "taskTerminalKey" ? undefined : value,
    );
}

function scheduleRetry(): void {
    if (retryTimer != null) return;
    retryTimer = window.setTimeout(() => {
        retryTimer = undefined;
        void startSaveLoop();
    }, RETRY_MS);
}

async function drainSaves(): Promise<boolean> {
    while (pendingSnapshot != null) {
        const current = pendingSnapshot;
        pendingSnapshot = null;
        activeSnapshot = current;
        try {
            await invoke("state_save", { data: current });
            lastSaved = current;
        } catch (error) {
            if (pendingSnapshot == null) pendingSnapshot = current;
            notify("error", `state save failed: ${errMessage(error)}; retrying`);
            scheduleRetry();
            return false;
        } finally {
            activeSnapshot = null;
        }
    }
    return true;
}

function startSaveLoop(): Promise<boolean> {
    if (saveLoop) return saveLoop;
    saveLoop = drainSaves().finally(() => {
        saveLoop = null;
    });
    return saveLoop;
}

function queueSnapshot(next: string): void {
    if (activeSnapshot != null) {
        // The active write will leave disk at activeSnapshot. If current state has
        // returned to that value, any previously queued newer value is obsolete.
        pendingSnapshot = next === activeSnapshot ? null : next;
        return;
    }
    pendingSnapshot = next === lastSaved ? null : next;
}

/** Save the latest state and wait until all currently queued writes have completed. */
export function flushPersist(): Promise<boolean> {
    if (persistTimer != null) {
        window.clearTimeout(persistTimer);
        persistTimer = undefined;
    }
    if (retryTimer != null) {
        window.clearTimeout(retryTimer);
        retryTimer = undefined;
    }
    lastSlices = takeSlices(getState());
    queueSnapshot(snapshot());
    return startSaveLoop();
}

/** Before v18 terminals died with the app, so a saved session id names nothing that could still run. */
function forgetCoreSessions(decoded: Record<string, unknown>): void {
    const windowsBySession = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    for (const rows of Object.values(windowsBySession)) {
        for (const row of Array.isArray(rows) ? rows : []) {
            const pending: unknown[] = isRecord(row) ? [row.root] : [];
            for (let node = pending.pop(); node !== undefined; node = pending.pop()) {
                if (!isRecord(node)) continue;
                delete node.ptyId;
                if (Array.isArray(node.children)) pending.push(...node.children);
            }
        }
    }
    const agentRows = [
        ...(Array.isArray(decoded.agents) ? decoded.agents : []),
        ...Object.values(isRecord(decoded.agentsBySession) ? decoded.agentsBySession : {}).flatMap((rows) => (Array.isArray(rows) ? rows : [])),
    ];
    for (const row of agentRows) if (isRecord(row)) delete row.ptyId;
}

/** Before v16 an agent's side pane held only its browser, as a "browser" pane whose saved state had no files. */
/** Before v17 GitHub had a session of its own. It lives in each project's git pane now, so that session has nothing left to show. */
function closeGithubSessions(decoded: Record<string, unknown>): void {
    const sessions = Array.isArray(decoded.sessions) ? decoded.sessions : [];
    const closed = new Set(sessions.flatMap((row) => (isRecord(row) && row.kind === "sikemux.github:hub" ? [row.id] : [])));
    if (closed.size === 0) return;
    decoded.sessions = sessions.filter((row) => !isRecord(row) || !closed.has(row.id));
    if (Array.isArray(decoded.sessionOrder)) decoded.sessionOrder = decoded.sessionOrder.filter((id) => !closed.has(id));
    if (isRecord(decoded.windowsBySession)) for (const id of closed) if (typeof id === "string") delete decoded.windowsBySession[id];
}

function moveBrowserPanesOntoDesks(decoded: Record<string, unknown>): void {
    const windowsBySession = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    for (const rows of Object.values(windowsBySession)) {
        for (const row of Array.isArray(rows) ? rows : []) {
            const pending: unknown[] = isRecord(row) ? [row.root] : [];
            for (let node = pending.pop(); node !== undefined; node = pending.pop()) {
                if (!isRecord(node)) continue;
                if (node.type === "pane" && node.kind === "browser") node.kind = "desk";
                else if (Array.isArray(node.children)) pending.push(...node.children);
            }
        }
    }
    const itemStates = isRecord(decoded.itemStates) ? decoded.itemStates : {};
    for (const envelope of Object.values(itemStates)) {
        if (!isRecord(envelope) || envelope.kind !== "browser") continue;
        envelope.kind = "desk";
        if (isRecord(envelope.state)) envelope.state.files = [];
    }
}

/** Before v10 Rundeck was built in, and its sessions, windows, panes and command contexts were plain "rundeck". */
const LEGACY_PLUGIN_KINDS: ReadonlyMap<unknown, string> = new Map([["rundeck", "sikemux.rundeck:deploy"]]);

function renameLegacyPluginKinds(decoded: Record<string, unknown>, kinds: ReadonlyMap<unknown, string> = LEGACY_PLUGIN_KINDS): void {
    const rename = (value: unknown) => kinds.get(value) ?? value;
    for (const row of Array.isArray(decoded.sessions) ? decoded.sessions : []) if (isRecord(row)) row.kind = rename(row.kind);
    const windowsBySession = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    for (const rows of Object.values(windowsBySession)) {
        for (const row of Array.isArray(rows) ? rows : []) {
            if (!isRecord(row)) continue;
            row.role = rename(row.role);
            const pending: unknown[] = [row.root];
            for (let node = pending.pop(); node !== undefined; node = pending.pop()) {
                if (!isRecord(node)) continue;
                if (node.type === "pane") node.kind = rename(node.kind);
                else if (Array.isArray(node.children)) for (const child of node.children) pending.push(child);
            }
        }
    }
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    for (const command of Array.isArray(prefs.customCommands) ? prefs.customCommands : []) {
        if (isRecord(command) && Array.isArray(command.contexts)) command.contexts = command.contexts.map(rename);
    }
}

/**
 * Before v13 AWS was built in. Its sessions, windows and panes were plain "aws",
 * its profile and service sat among core's settings, and its shortcut was core's.
 */
function moveAwsIntoItsPlugin(decoded: Record<string, unknown>): void {
    const windowsBySession = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    for (const rows of Object.values(windowsBySession)) {
        for (const row of Array.isArray(rows) ? rows : []) {
            if (isRecord(row) && row.role === undefined && row.name === "aws") row.role = "aws";
        }
    }
    renameLegacyPluginKinds(decoded, new Map([["aws", "sikemux.aws:console"]]));
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const pluginSettings = isRecord(prefs.pluginSettings) ? prefs.pluginSettings : {};
    const keybindingOverrides = isRecord(prefs.keybindingOverrides) ? { ...prefs.keybindingOverrides } : {};
    if ("aws.open" in keybindingOverrides) {
        keybindingOverrides["plugin.open:sikemux.aws"] = keybindingOverrides["aws.open"];
        delete keybindingOverrides["aws.open"];
    }
    decoded.prefs = {
        ...prefs,
        keybindingOverrides,
        pluginSettings: { ...pluginSettings, "sikemux.aws": { profile: prefs.awsProfile, service: prefs.awsService } },
    };
}

const LEGACY_BRUNO_SHORTCUTS: Readonly<Record<string, string>> = {
    "bruno.open": "plugin.open:sikemux.bruno",
    "bruno.save": "plugin.run:sikemux.bruno/save",
    "bruno.send": "plugin.run:sikemux.bruno/send",
    "bruno.environment": "plugin.run:sikemux.bruno/environment",
};

/**
 * Before v14 Bruno was built in. Its session kept the loaded collection and the
 * chosen environments, the workspace list sat among core's settings, and its
 * shortcuts were core's.
 */
function moveBrunoIntoItsPlugin(decoded: Record<string, unknown>): void {
    const sessions = Array.isArray(decoded.sessions) ? decoded.sessions : [];
    const session = sessions.find((row): row is Record<string, unknown> => isRecord(row) && row.kind === "bruno");
    const saved = session && isRecord(session.bruno) ? session.bruno : {};
    const collectionPath = typeof saved.collectionPath === "string" ? saved.collectionPath : typeof session?.cwd === "string" ? session.cwd : "";
    for (const row of sessions) if (isRecord(row)) delete row.bruno;

    const windowsBySession = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    for (const rows of Object.values(windowsBySession)) {
        for (const row of Array.isArray(rows) ? rows : []) {
            if (isRecord(row) && row.role === undefined && row.name === "bruno") row.role = "bruno";
        }
    }
    renameLegacyPluginKinds(decoded, new Map([["bruno", "sikemux.bruno:client"]]));

    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const workspaces = Array.isArray(prefs.brunoWorkspaces) ? prefs.brunoWorkspaces : [];
    const keybindingOverrides: Record<string, unknown> = {};
    for (const [id, binding] of Object.entries(isRecord(prefs.keybindingOverrides) ? prefs.keybindingOverrides : {})) {
        keybindingOverrides[LEGACY_BRUNO_SHORTCUTS[id] ?? id] = binding;
    }
    const { brunoWorkspaces: _moved, ...rest } = prefs;
    const pluginSettings = isRecord(prefs.pluginSettings) ? prefs.pluginSettings : {};
    decoded.prefs = {
        ...rest,
        keybindingOverrides,
        pluginSettings: {
            ...pluginSettings,
            "sikemux.bruno": {
                collectionPath,
                selectedEnvs: saved.selectedEnvs,
                workspaces: collectionPath ? [collectionPath, ...workspaces] : workspaces,
            },
        },
    };
}

/** Before v11 Rundeck's settings sat among core's, and each session kept the deploy location picked for its folder. */
function moveRundeckSettings(decoded: Record<string, unknown>): void {
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const deployTargets: Record<string, unknown> = {};
    for (const row of Array.isArray(decoded.sessions) ? decoded.sessions : []) {
        if (isRecord(row) && typeof row.cwd === "string" && row.cwd && isRecord(row.deploy)) deployTargets[row.cwd] = row.deploy;
    }
    const legacy = isRecord(prefs.rundeck) ? prefs.rundeck : {};
    const pluginSettings = isRecord(prefs.pluginSettings) ? prefs.pluginSettings : {};
    decoded.prefs = { ...prefs, pluginSettings: { ...pluginSettings, "sikemux.rundeck": { ...legacy, deployTargets } } };
}

/**
 * Before v15 Rundeck browsed one env folder and remembered a folder per project.
 * Now it browses any group path and remembers a job, which a folder can't name, so those picks are dropped.
 */
function reshapeRundeckSettings(decoded: Record<string, unknown>): void {
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const pluginSettings = isRecord(prefs.pluginSettings) ? prefs.pluginSettings : {};
    const saved = pluginSettings["sikemux.rundeck"];
    if (!isRecord(saved)) return;
    const { activeEnvFolder, deployTargets: _folders, ...rest } = saved;
    const activeGroup = typeof activeEnvFolder === "string" && activeEnvFolder ? activeEnvFolder : null;
    decoded.prefs = { ...prefs, pluginSettings: { ...pluginSettings, "sikemux.rundeck": { ...rest, activeGroup } } };
}

/**
 * Before v12 each Bruno workspace was its own session. Now one session switches
 * between them, so the first stays, the rest close, and every folder stays on
 * the list of workspaces.
 */
function mergeBrunoSessions(decoded: Record<string, unknown>): void {
    const sessions = Array.isArray(decoded.sessions) ? decoded.sessions : [];
    const [kept, ...extra] = sessions.filter((row): row is Record<string, unknown> => isRecord(row) && row.kind === "bruno");
    if (!kept) return;
    const folders = [kept, ...extra].flatMap((row) => {
        const path = isRecord(row.bruno) ? row.bruno.collectionPath : row.cwd;
        return typeof path === "string" && path ? [path] : [];
    });
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const saved = Array.isArray(prefs.brunoWorkspaces) ? prefs.brunoWorkspaces : [];
    decoded.prefs = { ...prefs, brunoWorkspaces: [...saved, ...folders] };

    const closed = new Set(extra.map((row) => row.id));
    decoded.sessions = sessions.filter((row) => !isRecord(row) || !closed.has(row.id));
    if (Array.isArray(decoded.sessionOrder)) decoded.sessionOrder = decoded.sessionOrder.filter((id) => !closed.has(id));
    if (isRecord(decoded.windowsBySession)) for (const id of closed) if (typeof id === "string") delete decoded.windowsBySession[id];
    if (closed.has(decoded.activeSessionId)) decoded.activeSessionId = kept.id;
}

function booleansByPath(value: unknown): Record<string, boolean> {
    if (!isRecord(value)) return {};
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"));
}

function normalisePluginSettings(value: unknown): Record<string, unknown> {
    if (!isRecord(value)) return {};
    return Object.fromEntries(Object.entries(value).filter(([id]) => isPluginId(id)));
}

export type HydrationResult = "empty" | "applied" | "invalid" | "unsupported-future";

export function hydrationAllowsPersistence(result: HydrationResult): boolean {
    return result === "empty" || result === "applied";
}

export function applyHydrate(raw: string): HydrationResult {
    if (!raw) return "empty";
    let decoded: unknown;
    try {
        decoded = JSON.parse(raw);
    } catch {
        return "invalid";
    }
    if (!isRecord(decoded) || typeof decoded.version !== "number" || !Number.isSafeInteger(decoded.version)) return "invalid";
    if (decoded.version > VERSION) return "unsupported-future";
    if (decoded.version < MIN_SUPPORTED_VERSION) return "invalid";
    if (!Array.isArray(decoded.sessions)) return "invalid";
    if (decoded.version < PLUGIN_KIND_MIGRATION_VERSION) renameLegacyPluginKinds(decoded);
    if (decoded.version < PLUGIN_SETTINGS_MIGRATION_VERSION) moveRundeckSettings(decoded);
    if (decoded.version < ONE_BRUNO_SESSION_MIGRATION_VERSION) mergeBrunoSessions(decoded);
    if (decoded.version < AWS_PLUGIN_MIGRATION_VERSION) moveAwsIntoItsPlugin(decoded);
    if (decoded.version < BRUNO_PLUGIN_MIGRATION_VERSION) moveBrunoIntoItsPlugin(decoded);
    if (decoded.version < RUNDECK_GROUPS_MIGRATION_VERSION) reshapeRundeckSettings(decoded);
    if (decoded.version < DESK_MIGRATION_VERSION) moveBrowserPanesOntoDesks(decoded);
    if (decoded.version < GITHUB_IN_GIT_PANE_MIGRATION_VERSION) closeGithubSessions(decoded);
    if (decoded.version < CORE_SESSION_MIGRATION_VERSION) forgetCoreSessions(decoded);

    const sessions: Record<string, Session> = {};
    for (const row of decoded.sessions) {
        const session = toSession(row);
        if (session && !sessions[session.id]) sessions[session.id] = session;
    }
    if (Object.keys(sessions).length === 0) return "invalid";

    const windows: Record<string, Window> = {};
    const agents: Record<string, Agent> = {};
    const windowsBySession: Record<string, string[]> = {};
    const rawWindows = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    const usedLayoutIds = new Set<string>();
    for (const sid of Object.keys(sessions)) {
        const rows = Array.isArray(rawWindows[sid]) ? rawWindows[sid] : [];
        windowsBySession[sid] = [];
        for (const row of rows) {
            if (!isWindow(row) || windows[row.id]) continue;
            const ids = layoutIds(row.root);
            if (new Set(ids.all).size !== ids.all.length || ids.all.some((id) => usedLayoutIds.has(id))) continue;
            ids.all.forEach((id) => usedLayoutIds.add(id));
            const restored: Window = {
                ...row,
                root: sessions[sid].kind === "ssh" ? upgradeSshStartup(row.root, sessions[sid].name) : row.root,
                role: deriveRole(row),
                activePaneId: ids.panes.includes(row.activePaneId) ? row.activePaneId : ids.panes[0],
            };
            if (sessions[sid].kind === "project") delete restored.fixed;
            if (sessions[sid].kind === "project" && restored.role === "term") restored.name = "Terminal";
            windows[row.id] = restored;
            windowsBySession[sid].push(row.id);
        }
    }
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const cur = getState();
    const providerProfiles = normaliseProviderProfiles(prefs.providerProfiles, cur.providerProfiles);
    const restoreAgentTabs = typeof prefs.restoreAgentTabs === "boolean" ? prefs.restoreAgentTabs : true;
    const spaces = normaliseSpaces(prefs.spaces);
    // Before v8 an agent sat beside its session rather than in a window, and
    // the session recorded which agent it was looking at. Each becomes a window
    // here, and that focus becomes the active window.
    const agentRows: Array<{ sid: string | null; row: unknown }> =
        decoded.version >= 8
            ? (Array.isArray(decoded.agents) ? decoded.agents : []).map((row) => ({ sid: null, row }))
            : Object.entries(isRecord(decoded.agentsBySession) ? decoded.agentsBySession : {}).flatMap(([sid, rows]) =>
                  Array.isArray(rows) ? rows.map((row) => ({ sid, row })) : [],
              );
    const legacyAgentFocus = new Map<string, string>();
    if (decoded.version < 8) {
        for (const row of decoded.sessions) {
            if (isRecord(row) && typeof row.id === "string" && row.view === "agent" && typeof row.activeAgentId === "string") {
                legacyAgentFocus.set(row.id, row.activeAgentId);
            }
        }
    }
    const claimedResumeIds = new Set<string>();
    if (restoreAgentTabs) {
        for (const { sid, row } of agentRows) {
            if (sid !== null && sessions[sid]?.kind !== "project") continue;
            const saved = toPersistedAgent(row);
            if (!saved || agents[saved.id]) continue;
            if (saved.resumeId) {
                const claim = `${saved.type}\0${saved.resumeId}`;
                if (claimedResumeIds.has(claim)) continue;
                claimedResumeIds.add(claim);
            }
            const permissionMode = normalizePermissionMode(
                saved.type,
                saved.permissionMode ?? (saved.skipPermissions ? "bypass" : "workspace-write"),
            );
            const profile = saved.profileId
                ? providerProfiles.find((item) => item.id === saved.profileId && item.provider === saved.type)
                : undefined;
            const executablePath = profile?.executablePath || saved.executablePath;
            const launchOptions = {
                model: saved.model,
                effort: saved.effort,
                configPath: profile?.configPath,
                environmentKeys: profile?.environmentKeys,
            };
            if (saved.profileId && !providerProfiles.some((profile) => profile.id === saved.profileId && profile.provider === saved.type)) {
                delete saved.profileId;
            }
            // Startup is rebuilt from the trusted type/resume id pair, never read from disk.
            agents[saved.id] = {
                ...saved,
                permissionMode,
                executablePath,
                startup: agentStartup(saved.type, saved.resumeId, permissionMode, executablePath, launchOptions),
                directCommand: agentDirectCommand(saved.type, saved.resumeId, permissionMode, executablePath, launchOptions),
                launchState: "dormant",
            };
            if (sid !== null) {
                const session = sessions[sid];
                const win = agentWindow(agents[saved.id], saved.cwd ?? session.cwd);
                windows[win.id] = win;
                windowsBySession[sid].push(win.id);
                if (legacyAgentFocus.get(sid) === saved.id) sessions[sid] = { ...session, activeWindowId: win.id };
            }
        }
    }
    // An agent window whose record did not come back has nothing to show.
    for (const sid of Object.keys(sessions)) {
        windowsBySession[sid] = windowsBySession[sid].filter((id) => {
            const win = windows[id];
            if (win?.role !== "agent") return true;
            if (sessions[sid].kind === "project" && agents[agentPaneId(win) ?? ""]) return true;
            delete windows[id];
            return false;
        });
        const session = sessions[sid];
        const windowIds = windowsBySession[sid];
        sessions[sid] = { ...session, activeWindowId: windowIds.includes(session.activeWindowId) ? session.activeWindowId : (windowIds[0] ?? "") };
    }

    const panesById = new Map<string, ReturnType<typeof workbenchItemRefFromPane>>();
    for (const w of Object.values(windows)) {
        const walk = (n: Window["root"]): void => {
            if (n.type === "pane") panesById.set(n.id, workbenchItemRefFromPane(n));
            else n.children.forEach(walk);
        };
        walk(w.root);
    }
    const editorViews: Record<string, EditorPaneView> = {};
    const deskPanes: Record<string, string> = {};
    const deskRestores: Record<string, DeskView> = {};
    const desks: Record<string, Desk> = {};
    if (decoded.version >= 7) {
        const rawItemStates = isRecord(decoded.itemStates) ? decoded.itemStates : {};
        for (const [itemId, ref] of panesById) {
            const result = workbenchItemRegistry.decodePersisted(ref, rawItemStates[itemId]);
            if (!result.ok) continue;
            if (result.ref.kind === "editor") editorViews[itemId] = result.state as EditorPaneView;
            if (result.ref.kind === "desk") {
                const view = result.state as DeskView;
                // Without its agent the pane has nothing to be, and the pane
                // itself takes the empty leaf back out of the layout.
                if (!agents[view.agentId]) continue;
                deskPanes[itemId] = view.agentId;
                deskRestores[itemId] = view;
                editorViews[deskEditorId(view.agentId)] = { openTabs: view.files, activePath: view.files[0] ?? null };
                desks[view.agentId] = {
                    order: view.files.map(fileKey),
                    active: view.tabs.length > 0 ? BROWSER_ACTIVE : view.files[0] ? fileKey(view.files[0]) : null,
                    terminals: [],
                    simulators: [],
                    reveal: null,
                };
            }
        }
    } else {
        const rawEditorViews = isRecord(decoded.editorViews) ? decoded.editorViews : {};
        for (const [itemId, ref] of panesById) {
            if (ref.kind !== "editor" || !(itemId in rawEditorViews)) continue;
            const result = workbenchItemRegistry.decodePersisted(ref, {
                itemId,
                kind: "editor",
                version: workbenchItemRegistry.get("editor").persisted.version,
                state: rawEditorViews[itemId],
            });
            if (result.ok) editorViews[itemId] = result.state as EditorPaneView;
        }
    }

    const requestedOrder = Array.isArray(decoded.sessionOrder) ? decoded.sessionOrder.filter((id): id is string => typeof id === "string") : [];
    const sessionOrder = [...new Set(requestedOrder.filter((id) => sessions[id]))];
    for (const sid of Object.keys(sessions)) if (!sessionOrder.includes(sid)) sessionOrder.push(sid);
    const requestedActive = typeof decoded.activeSessionId === "string" ? decoded.activeSessionId : "";
    const activeSessionId = sessions[requestedActive] ? requestedActive : sessionOrder[0];

    setState({
        sessions,
        windows,
        agents,
        sessionOrder,
        windowsBySession,
        agentActivity: {},
        agentBackgroundWork: {},
        agentSubagents: {},
        activeSessionId,
        recent: Array.isArray(decoded.recent) ? decoded.recent.filter(isRecent) : [],
        editorViews,
        deskPanes,
        deskRestores,
        desks,
        browserStrips: {},
        // Pinned projects used to be their own list; they are self-indexed
        // roots now, folded in here so existing setups carry over untouched.
        projectRoots: mergePinnedIntoRoots(
            Array.isArray(prefs.projectRoots) ? normaliseProjectRoots(prefs.projectRoots) : cur.projectRoots,
            prefs.pinnedProjects,
        ),
        themeId: typeof prefs.themeId === "string" ? prefs.themeId : cur.themeId,
        customThemes: Array.isArray(prefs.customThemes) ? prefs.customThemes.filter(isTheme) : cur.customThemes,
        uiTextScale: typeof prefs.uiTextScale === "number" && [1, 1.1, 1.25].includes(prefs.uiTextScale) ? prefs.uiTextScale : cur.uiTextScale,
        paneShader: typeof prefs.paneShader === "boolean" ? prefs.paneShader : cur.paneShader,
        paneImage: typeof prefs.paneImage === "string" || prefs.paneImage === null ? prefs.paneImage : cur.paneImage,
        terminalFontSize: typeof prefs.terminalFontSize === "number" ? clampTerminalFontSize(prefs.terminalFontSize) : cur.terminalFontSize,
        chatTextScale: typeof prefs.chatTextScale === "number" ? clampChatTextScale(prefs.chatTextScale) : cur.chatTextScale,
        editorTextScale: typeof prefs.editorTextScale === "number" ? clampEditorTextScale(prefs.editorTextScale) : cur.editorTextScale,
        windowOpacity: typeof prefs.windowOpacity === "number" && Number.isFinite(prefs.windowOpacity) ? prefs.windowOpacity : cur.windowOpacity,
        windowBlur: typeof prefs.windowBlur === "number" && Number.isFinite(prefs.windowBlur) ? prefs.windowBlur : cur.windowBlur,
        cloudBrowser: typeof prefs.cloudBrowser === "string" ? prefs.cloudBrowser : cur.cloudBrowser,
        cloudBrowserShortcut: typeof prefs.cloudBrowserShortcut === "string" ? prefs.cloudBrowserShortcut : cur.cloudBrowserShortcut,
        keybindingOverrides: normaliseKeybindingOverrides(prefs.keybindingOverrides),
        sideRailOpen: typeof prefs.sideRailOpen === "boolean" ? prefs.sideRailOpen : cur.sideRailOpen,
        agentRailOpen: typeof prefs.agentRailOpen === "boolean" ? prefs.agentRailOpen : cur.agentRailOpen,
        sideRailWidth:
            typeof prefs.sideRailWidth === "number" && Number.isFinite(prefs.sideRailWidth)
                ? clampRailWidth("start", prefs.sideRailWidth)
                : cur.sideRailWidth,
        agentRailWidth:
            typeof prefs.agentRailWidth === "number" && Number.isFinite(prefs.agentRailWidth)
                ? clampRailWidth("end", prefs.agentRailWidth)
                : cur.agentRailWidth,
        pluginSettings: normalisePluginSettings(prefs.pluginSettings),
        disabledPlugins: Array.isArray(prefs.disabledPlugins) ? [...new Set(prefs.disabledPlugins.filter(isPluginId))] : [],
        restoreAgentTabs,
        spaces,
        projectSpaces: normaliseProjectSpaces(prefs.projectSpaces, spaces),
        activeSpaceId: spaces.some((space) => space.id === prefs.activeSpaceId) ? (prefs.activeSpaceId as string) : null,
        agentNotifications: typeof prefs.agentNotifications === "boolean" ? prefs.agentNotifications : cur.agentNotifications,
        notch: normaliseNotchSettings(prefs.notch),
        voiceDictation: prefs.voiceDictation === true,
        iosSimulator: typeof prefs.iosSimulator === "boolean" ? prefs.iosSimulator : cur.iosSimulator,
        notificationsIntroduced: prefs.notificationsIntroduced === true,
        keptRunningNoticeShown: prefs.keptRunningNoticeShown === true,
        railDensity: prefs.railDensity === "compact" || prefs.railDensity === "comfortable" ? prefs.railDensity : cur.railDensity,
        agentRailAllAgents: prefs.agentRailAllAgents === true,
        agentRailScope: prefs.agentRailScope === "all" ? "all" : "project",
        onboardingComplete:
            typeof prefs.onboardingComplete === "boolean"
                ? prefs.onboardingComplete
                : decoded.version < ONBOARDING_MIGRATION_VERSION
                  ? true
                  : cur.onboardingComplete,
        lastSeenVersion: typeof prefs.lastSeenVersion === "string" ? prefs.lastSeenVersion : cur.lastSeenVersion,
        customCommands: normaliseCustomCommands(prefs.customCommands),
        updateChannel: prefs.updateChannel === "nightly" || prefs.updateChannel === "stable" ? prefs.updateChannel : cur.updateChannel,
        shareUsageData: typeof prefs.shareUsageData === "boolean" ? prefs.shareUsageData : cur.shareUsageData,
        lastReleaseNotes:
            isRecord(prefs.lastReleaseNotes) && typeof prefs.lastReleaseNotes.version === "string"
                ? {
                      version: prefs.lastReleaseNotes.version,
                      notes: typeof prefs.lastReleaseNotes.notes === "string" ? prefs.lastReleaseNotes.notes : null,
                      date: typeof prefs.lastReleaseNotes.date === "string" ? prefs.lastReleaseNotes.date : null,
                      credits: parseReleaseCredits(prefs.lastReleaseNotes.credits),
                  }
                : null,
        recentCommandKeys: Array.isArray(prefs.recentCommandKeys)
            ? prefs.recentCommandKeys.filter((value): value is string => typeof value === "string").slice(0, 20)
            : [],
        providerProfiles,
        selectedProviderProfileIds: normaliseProviderProfileSelection(
            prefs.selectedProviderProfileIds,
            providerProfiles,
            cur.selectedProviderProfileIds,
        ),
        // Every save wrote this boundary out, so an older snapshot cannot say
        // whether it was chosen or just inherited. Adopt today's default once.
        defaultAgentPermissionMode:
            prefs.defaultAgentPermissionMode === undefined || decoded.version < AGENT_PERMISSION_DEFAULT_MIGRATION_VERSION
                ? cur.defaultAgentPermissionMode
                : prefs.defaultAgentPermissionMode === "bypass"
                  ? "bypass"
                  : "workspace-write",
        lastAgentType: AGENT_TYPES.has(prefs.lastAgentType as AgentType) ? (prefs.lastAgentType as AgentType) : null,
        languageServerTrust: booleansByPath(prefs.languageServerTrust),
        agentWorktreeDefaults: booleansByPath(prefs.agentWorktreeDefaults),
    });
    pruneOnDemandWindows();
    registerCustomThemes(getState().customThemes);
    // Preserve the actual disk payload as the saved marker. The subscription
    // rewrites migrations and sanitized legacy credentials in canonical v8 form.
    lastSaved = raw;
    lastSlices = takeSlices(getState());
    return "applied";
}

export function canFlushPersist(): boolean {
    return persistenceReady;
}

export function subscribePersist(): () => void {
    persistenceReady = true;
    queueSnapshot(snapshot());
    void startSaveLoop();
    const unsubscribe = useStore.subscribe((state, previous) => {
        if (slicesEqual(state, previous)) return;
        if (persistTimer != null) window.clearTimeout(persistTimer);
        persistTimer = window.setTimeout(() => {
            persistTimer = undefined;
            const slices = takeSlices(getState());
            if (lastSlices && slicesEqual(lastSlices, slices)) return;
            lastSlices = slices;
            queueSnapshot(snapshot());
            void startSaveLoop();
        }, 600);
    });
    let closed = false;
    return () => {
        if (closed) return;
        closed = true;
        unsubscribe();
        void flushPersist();
        persistenceReady = false;
    };
}

export function resetPersistenceForTests(): void {
    if (persistTimer != null) window.clearTimeout(persistTimer);
    if (retryTimer != null) window.clearTimeout(retryTimer);
    persistTimer = undefined;
    retryTimer = undefined;
    lastSaved = "";
    activeSnapshot = null;
    pendingSnapshot = null;
    saveLoop = null;
    lastSlices = null;
    persistenceReady = false;
}
