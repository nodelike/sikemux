import { normaliseNotchSettings } from "../notch/notchSettings";
import { invokeCommand as invoke } from "../api/invoke";
import { fixedSessionName } from "./sessionNames";
import { sshStartup } from "../terminal/sshStartup";
import { clampTerminalFontSize } from "../terminal/fontSize";
import { clampChatTextScale } from "../chat/textScale";
import { clampEditorTextScale } from "../editor/textScale";
import { isTheme } from "../themes";
import { parseReleaseCredits } from "../api/releases";
import type { AgentAccountBook } from "../api/agents";
import type { ProjectInfo, Workspace } from "../api/generated/wire";
import { normaliseKeybindingOverrides } from "../commands/keybindings";
import type { CommandContext, CustomCommand, CustomCommandPlacement } from "../commands/registry";
import { registerCustomThemes } from "../themes/bus";
import { ACCOUNT_AGENT_TYPES, AGENT_TYPES, hasAccounts, isAgentType, type AccountAgentType } from "../agents/catalog";
import { clampRailWidth } from "../lib/railWidths";
import { mergePinnedIntoRoots, normaliseProjectRoots, pruneOnDemandWindows } from "./commands";
import { clampPaneStrength } from "./commands/settings";
import { agentFromCore, sharedFromCore } from "./coreWorkspace";
import { agentInWindow, agentPaneId, chatKey } from "./selectors";
import { collectPanes, newId, removePane } from "./layout";
import { BROWSER_ACTIVE, deskEditorId, deskView, fileKey, simulatorKey } from "./desks";
import { agentExecutablePath } from "./commands/agentLaunchCommand";
import { agentWindow } from "./agentWindow";
import { getState, setState, useStore, type StoreState } from "./store";
import { notify } from "./toast";
import { errorText } from "../lib/format";
import { isSessionKind, validatePersistedLayout } from "./persistValidation";
import { isRecord } from "../lib/guards";
import { restoredHostChoice, savedHostChoice, type HostChoice } from "../hosts/hosts";
import { isWindowRole } from "./windowRoles";
import { isPluginId } from "../plugins/kinds";
import { decodeItemState, encodeItemState, type PersistedWorkbenchItemEnvelope } from "../workbench/itemStates";
import type {
    Agent,
    Desk,
    DeskView,
    EditorPaneView,
    LayoutNode,
    ProviderProfile,
    ProviderProfileSelection,
    RecentEntry,
    Session,
    Window,
} from "./types";

export const VERSION = 21;
const MIN_SUPPORTED_VERSION = 4;
const ONBOARDING_MIGRATION_VERSION = 6;
const ITEM_STATES_MIGRATION_VERSION = 7;
const SELF_INDEXED_ROOTS_MIGRATION_VERSION = 9;
const PLUGIN_KIND_MIGRATION_VERSION = 10;
const PLUGIN_SETTINGS_MIGRATION_VERSION = 11;
const ONE_BRUNO_SESSION_MIGRATION_VERSION = 12;
const AWS_PLUGIN_MIGRATION_VERSION = 13;
const BRUNO_PLUGIN_MIGRATION_VERSION = 14;
const RUNDECK_GROUPS_MIGRATION_VERSION = 15;
const DESK_MIGRATION_VERSION = 16;
const GITHUB_IN_GIT_PANE_MIGRATION_VERSION = 17;
const CORE_SESSION_MIGRATION_VERSION = 18;
const SSH_LOOP_MIGRATION_VERSION = 18;
const PANE_TITLE_MIGRATION_VERSION = 20;
const TAB_RAIL_MIGRATION_VERSION = 21;
const RETRY_MS = 1500;
let lastSaved = "";
let activeSnapshot: string | null = null;
let pendingSnapshot: string | null = null;
let saveLoop: Promise<boolean> | null = null;
let retryTimer: number | undefined;
let persistTimer: number | undefined;
let persistenceReady = false;

type Read<T> = (saved: unknown, current: T) => T;

const flag: Read<boolean> = (saved, current) => (typeof saved === "boolean" ? saved : current);
const text: Read<string> = (saved, current) => (typeof saved === "string" ? saved : current);
const shortName: Read<string> = (saved, current) => boundedOptionalString(saved, 40) ?? current;
const finite =
    (clamp: (value: number) => number = (value) => value): Read<number> =>
    (saved, current) =>
        typeof saved === "number" && Number.isFinite(saved) ? clamp(saved) : current;
const oneOf =
    <T extends string | number>(...values: readonly T[]): Read<T> =>
    (saved, current) =>
        values.includes(saved as T) ? (saved as T) : current;

function booleansByPath(value: unknown): Record<string, boolean> {
    if (!isRecord(value)) return {};
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"));
}

function stringsByPath(value: unknown): Record<string, string> {
    if (!isRecord(value)) return {};
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
}

function normalisePluginSettings(value: unknown): Record<string, unknown> {
    if (!isRecord(value)) return {};
    return Object.fromEntries(Object.entries(value).filter(([id]) => isPluginId(id)));
}

function readReleaseNotes(saved: unknown): StoreState["lastReleaseNotes"] {
    if (!isRecord(saved) || typeof saved.version !== "string") return null;
    return {
        version: saved.version,
        notes: typeof saved.notes === "string" ? saved.notes : null,
        date: typeof saved.date === "string" ? saved.date : null,
        credits: parseReleaseCredits(saved.credits),
    };
}

/** Each setting this window saves, and how a saved value reads back: one it cannot read keeps the store's default. */
const PREFS = {
    projectRoots: (saved, current) => (Array.isArray(saved) ? normaliseProjectRoots(saved) : current),
    themeId: text,
    customThemes: (saved, current) => (Array.isArray(saved) ? saved.filter(isTheme) : current),
    uiTextScale: oneOf<number>(1, 1.1, 1.25),
    paneShader: flag,
    buddies: flag,
    buddyPack: shortName,
    buddyShape: shortName,
    buddyMotion: flag,
    paneImage: (saved, current) => (typeof saved === "string" || saved === null ? saved : current),
    paneStrength: finite(clampPaneStrength),
    terminalFontSize: finite(clampTerminalFontSize),
    chatTextScale: finite(clampChatTextScale),
    editorTextScale: finite(clampEditorTextScale),
    windowOpacity: finite(),
    windowBlur: finite(),
    cloudBrowser: text,
    cloudBrowserShortcut: text,
    keybindingOverrides: (saved) => normaliseKeybindingOverrides(saved),
    sideRailOpen: flag,
    tabRailOpen: flag,
    sideRailWidth: finite((width) => clampRailWidth("start", width)),
    tabRailWidth: finite((width) => clampRailWidth("end", width)),
    pluginSettings: (saved) => normalisePluginSettings(saved),
    restoreAgentTabs: flag,
    activeSpaceId: (saved, current) => (typeof saved === "string" ? saved : current),
    agentNotifications: flag,
    notch: (saved) => normaliseNotchSettings(saved),
    voiceDictation: flag,
    iosSimulator: flag,
    simulatorFrame: flag,
    notificationsIntroduced: flag,
    keptRunningNoticeShown: flag,
    signInCardDismissed: flag,
    railDensity: oneOf("comfortable", "compact"),
    midTurnSend: oneOf("steer", "queue"),
    tabRailAllAgents: flag,
    tabRailScope: oneOf("project", "all"),
    tabRailPage: oneOf("agents", "terminals", "files"),
    onboardingComplete: flag,
    lastSeenVersion: text,
    customCommands: (saved) => normaliseCustomCommands(saved),
    updateChannel: oneOf("stable", "nightly"),
    shareUsageData: flag,
    lastReleaseNotes: (saved) => readReleaseNotes(saved),
    recentCommandKeys: (saved, current) =>
        Array.isArray(saved) ? saved.filter((value): value is string => typeof value === "string").slice(0, 20) : current,
    lastAgentType: (saved, current) => (isAgentType(saved) ? saved : current),
    languageServerTrust: (saved) => booleansByPath(saved),
    agentWorktreeDefaults: (saved) => booleansByPath(saved),
    defaultGitRepos: (saved) => stringsByPath(saved),
} satisfies { [K in keyof StoreState]?: Read<StoreState[K]> };

type PrefKey = keyof typeof PREFS;
type PersistedPrefs = { [K in PrefKey]: StoreState[K] };
const PREF_KEYS = Object.keys(PREFS) as PrefKey[];

const PERSISTED_KEYS = [
    "sessions",
    "windows",
    "agents",
    "sessionOrder",
    "windowsBySession",
    "activeSessionId",
    "chosenHost",
    "recent",
    "editorViews",
    "deskPanes",
    "browserStrips",
    "deskRestores",
    ...PREF_KEYS,
] satisfies readonly (keyof StoreState)[];
type SliceShot = Partial<Record<keyof StoreState, unknown>>;
let lastSlices: SliceShot | null = null;

function takeSlices(s: StoreState): SliceShot {
    return Object.fromEntries(PERSISTED_KEYS.map((key) => [key, s[key]]));
}

function slicesEqual(a: SliceShot | StoreState, b: SliceShot | StoreState): boolean {
    return PERSISTED_KEYS.every((key) => a[key] === b[key]);
}

function packPrefs(s: StoreState): PersistedPrefs {
    return Object.fromEntries(PREF_KEYS.map((key) => [key, s[key]])) as PersistedPrefs;
}

function readPrefs(saved: Record<string, unknown>, current: StoreState): PersistedPrefs {
    return Object.fromEntries(PREF_KEYS.map((key) => [key, (PREFS[key] as Read<unknown>)(saved[key], current[key])])) as PersistedPrefs;
}

/** A project's name and folder are the core's, so a window keeps only which of its tabs is in front. */
type PersistedSession = Session | Pick<Session, "id" | "kind" | "activeWindowId">;

export interface PersistedSnapshot {
    version: number;
    sessions: PersistedSession[];
    windowsBySession: Record<string, Window[]>;
    sessionOrder: string[];
    activeSessionId: string;
    /** The host chosen in the side rail over the active session, when one was. */
    chosenHost?: HostChoice;
    recent: RecentEntry[];
    prefs: Partial<PersistedPrefs>;
    /** Versioned item-owned state. Live processes, buffers, and credentials are excluded. */
    itemStates: Record<string, PersistedWorkbenchItemEnvelope>;
    /** The chats open here that run on other hosts, which this host's core does not keep. */
    hostAgents?: Agent[];
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
        isWindowRole(value.role) &&
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
    if (typeof value.host === "string") session.host = value.host;
    return session;
}

/** A chat on another host as this window kept it, which takes it up there again when shown. */
function hostAgent(value: unknown): Agent | null {
    if (!isRecord(value) || typeof value.id !== "string" || typeof value.host !== "string" || !isAgentType(value.type)) return null;
    if (typeof value.title !== "string" || typeof value.cwd !== "string") return null;
    return {
        id: value.id,
        type: value.type,
        title: value.title,
        name: typeof value.name === "string" ? value.name : undefined,
        cwd: value.cwd,
        host: value.host,
        createdAt: typeof value.createdAt === "number" ? value.createdAt : Date.now(),
        permissionMode: value.permissionMode === "bypass" ? "bypass" : "workspace-write",
        launchState: "live",
    };
}

function isRecent(value: unknown): value is RecentEntry {
    return isRecord(value) && isSessionKind(value.kind) && typeof value.name === "string" && typeof value.cwd === "string";
}

function normaliseProviderProfiles(value: unknown, fallback: ProviderProfile[]): ProviderProfile[] {
    if (!Array.isArray(value)) return fallback.map((profile) => ({ ...profile }));
    const profiles: ProviderProfile[] = [];
    const seen = new Set<string>();
    for (const row of value) {
        if (!isRecord(row) || typeof row.id !== "string" || !row.id.trim() || seen.has(row.id)) continue;
        if (typeof row.name !== "string" || !row.name.trim() || !hasAccounts(row.provider as string)) continue;
        if (typeof row.accent !== "string" || !/^#[\da-f]{6}$/i.test(row.accent)) continue;
        const id = row.id.trim().slice(0, 100);
        if (seen.has(id)) continue;
        const profile: ProviderProfile = {
            id,
            name: row.name.trim().slice(0, 100),
            provider: row.provider as AccountAgentType,
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

function normaliseAccountAutoSwitch(value: unknown): StoreState["accountAutoSwitch"] {
    const saved = value && typeof value === "object" ? (value as Record<string, unknown>) : {};
    const result: StoreState["accountAutoSwitch"] = {};
    for (const provider of ACCOUNT_AGENT_TYPES) if (saved[provider] === true) result[provider] = true;
    return result;
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
        if (profile && hasAccounts(type) && profile.provider === type) selection[type] = selected;
    }
    return selection;
}

function boundedOptionalString(value: unknown, max: number): string | undefined {
    return typeof value === "string" && value.trim() && !/[\0\r\n]/.test(value) ? value.slice(0, max) : undefined;
}

/** A project's name and folder are the core's; this window keeps only which of its tabs is in front. */
function persistedSession(sess: Session): PersistedSession {
    return sess.kind === "project" && !sess.host ? { id: sess.id, kind: sess.kind, activeWindowId: sess.activeWindowId } : sess;
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
    if (window.role === "agent" && !agentInWindow(s.agents, window)) return null;
    const kept = withoutEmptyDesks(window);
    return kept && { ...kept, root: withoutTaskBinding(kept.root) };
}

/** A task's terminal moved into a saved tab comes back as a plain terminal: the task process it showed is gone by then. */
function withoutTaskBinding(node: LayoutNode): LayoutNode {
    if (node.type === "split") return { ...node, children: node.children.map(withoutTaskBinding) };
    if (!node.externalPty && node.taskTerminalKey === undefined) return node;
    const { externalPty: _external, taskTerminalKey: _key, ...pane } = node;
    return pane;
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
    const itemStates: PersistedSnapshot["itemStates"] = {};
    for (const sess of sessions) {
        windowsBySession[sess.id] = (s.windowsBySession[sess.id] ?? []).flatMap((id) => {
            const window = durableWindow(s, id);
            return window ? [window] : [];
        });
        for (const window of windowsBySession[sess.id]) {
            const pending = [window.root];
            while (pending.length > 0) {
                const node = pending.pop()!;
                if (node.type === "split") {
                    pending.push(...node.children);
                    continue;
                }
                if (node.kind === "editor")
                    itemStates[node.id] = encodeItemState(node.id, "editor", s.editorViews[node.id] ?? { openTabs: [], activePath: null });
                if (node.kind === "desk") {
                    const view = deskView(node.id);
                    if (view) itemStates[node.id] = encodeItemState(node.id, "desk", view);
                }
            }
        }
    }
    const snap: PersistedSnapshot = {
        version: VERSION,
        sessions,
        windowsBySession,
        sessionOrder: sessions.map((s) => s.id),
        activeSessionId: s.activeSessionId,
        chosenHost: savedHostChoice(s),
        recent: s.recent,
        prefs: packPrefs(s),
        itemStates,
        hostAgents: Object.values(s.agents).filter((agent) => agent.host),
    };
    return JSON.stringify(snap);
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
            notify("error", `state save failed: ${errorText(error)}; retrying`);
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

function eachSavedPane(decoded: Record<string, unknown>, visit: (pane: Record<string, unknown>, session: string) => void): void {
    const windowsBySession = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    for (const [session, rows] of Object.entries(windowsBySession)) {
        for (const row of Array.isArray(rows) ? rows : []) {
            const pending: unknown[] = isRecord(row) ? [row.root] : [];
            for (let node = pending.pop(); node !== undefined; node = pending.pop()) {
                if (!isRecord(node)) continue;
                if (node.type === "pane") visit(node, session);
                else if (Array.isArray(node.children)) pending.push(...node.children);
            }
        }
    }
}

/** Before v6 there was no welcome screen, so anyone with saved state had already started. */
function skipOnboarding(decoded: Record<string, unknown>): void {
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    if (typeof prefs.onboardingComplete !== "boolean") decoded.prefs = { ...prefs, onboardingComplete: true };
}

/** Before v7 an editor's open files sat in their own map instead of in the item states. */
function moveEditorViewsIntoItemStates(decoded: Record<string, unknown>): void {
    const editorViews = isRecord(decoded.editorViews) ? decoded.editorViews : {};
    const itemStates: Record<string, unknown> = {};
    for (const [itemId, state] of Object.entries(editorViews)) {
        itemStates[itemId] = encodeItemState(itemId, "editor", state as EditorPaneView);
    }
    decoded.itemStates = itemStates;
    delete decoded.editorViews;
}

/** Before v9 pinned projects were a list of their own; each is a root indexed as itself now. */
function foldPinnedProjectsIntoRoots(decoded: Record<string, unknown>): void {
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const roots = Array.isArray(prefs.projectRoots) ? normaliseProjectRoots(prefs.projectRoots) : [];
    const { pinnedProjects, ...rest } = prefs;
    decoded.prefs = { ...rest, projectRoots: mergePinnedIntoRoots(roots, pinnedProjects) };
}

/** Before v18 an SSH tab ran `ssh` itself, or a reconnect loop in the login shell; it runs the loop under /bin/sh now. */
function upgradeSshStartups(decoded: Record<string, unknown>): void {
    const sessions = Array.isArray(decoded.sessions) ? decoded.sessions : [];
    const aliases = new Map(
        sessions.flatMap((row) => (isRecord(row) && row.kind === "ssh" && typeof row.name === "string" ? [[row.id, row.name]] : [])),
    );
    eachSavedPane(decoded, (pane, session) => {
        const alias = aliases.get(session);
        if (alias === undefined || pane.kind !== "terminal" || typeof pane.startup !== "string") return;
        if (pane.startup === `ssh ${alias}` || pane.startup.includes("sikemux_ssh_retries")) pane.startup = sshStartup(alias);
    });
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
}

const PANE_TITLE_RENAMES: Record<string, string> = {
    shell: "Terminal",
    editor: "Files",
    git: "Git",
    diff: "Diff",
    search: "Search",
    agent: "Agent",
    desk: "Desk",
    "shared chat": "Shared chat",
};

function capitalisePaneTitles(decoded: Record<string, unknown>): void {
    const windowsBySession = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    for (const rows of Object.values(windowsBySession)) {
        for (const row of Array.isArray(rows) ? rows : []) {
            const pending: unknown[] = isRecord(row) ? [row.root] : [];
            for (let node = pending.pop(); node !== undefined; node = pending.pop()) {
                if (!isRecord(node)) continue;
                if (node.type === "pane" && typeof node.title === "string") node.title = PANE_TITLE_RENAMES[node.title] ?? node.title;
                if (Array.isArray(node.children)) pending.push(...node.children);
            }
        }
    }
}

function renameAgentRailPrefs(decoded: Record<string, unknown>): void {
    if (!isRecord(decoded.prefs)) return;
    const prefs = decoded.prefs;
    for (const key of Object.keys(prefs)) {
        if (!key.startsWith("agentRail")) continue;
        prefs[`tabRail${key.slice("agentRail".length)}`] = prefs[key];
        delete prefs[key];
    }
}

/** Before v17 GitHub had a session of its own. It lives in each project's git pane now, so that session has nothing left to show. */
function closeGithubSessions(decoded: Record<string, unknown>): void {
    const sessions = Array.isArray(decoded.sessions) ? decoded.sessions : [];
    const closed = new Set(sessions.flatMap((row) => (isRecord(row) && row.kind === "sikemux.github:hub" ? [row.id] : [])));
    if (closed.size === 0) return;
    decoded.sessions = sessions.filter((row) => !isRecord(row) || !closed.has(row.id));
    if (Array.isArray(decoded.sessionOrder)) decoded.sessionOrder = decoded.sessionOrder.filter((id) => !closed.has(id));
    if (isRecord(decoded.windowsBySession)) for (const id of closed) if (typeof id === "string") delete decoded.windowsBySession[id];
}

/** Before v16 an agent's side pane held only its browser, as a "browser" pane whose saved state had no files. */
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

export type HydrationResult = "empty" | "applied" | "invalid" | "unsupported-future";

export function hydrationAllowsPersistence(result: HydrationResult): boolean {
    return result === "empty" || result === "applied";
}

export function adoptAgentAccounts(book: AgentAccountBook): void {
    const providerProfiles = normaliseProviderProfiles(book.profiles, []);
    setState({
        providerProfiles,
        selectedProviderProfileIds: normaliseProviderProfileSelection(book.selected, providerProfiles, {}),
        accountAutoSwitch: normaliseAccountAutoSwitch(book.autoSwitch),
    });
}

/** Applies the saved workspace, after the agent accounts the core keeps, which restoring its agents needs. */
function projectSession(project: ProjectInfo, activeWindowId: string): Session {
    return { id: project.id, name: project.name, kind: "project", cwd: project.path, pinned: project.pinned, activeWindowId };
}

/**
 * Restores what this window saved of its own, around the workspace the core keeps: the projects, agents, spaces and
 * pinned chats come from `workspace`, and the tabs, layout and settings from `raw`.
 */
export function applyHydrate(raw: string, accounts?: AgentAccountBook | null, workspace?: Workspace): HydrationResult {
    if (accounts) adoptAgentAccounts(accounts);
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
    if (decoded.version < ONBOARDING_MIGRATION_VERSION) skipOnboarding(decoded);
    if (decoded.version < ITEM_STATES_MIGRATION_VERSION) moveEditorViewsIntoItemStates(decoded);
    if (decoded.version < SELF_INDEXED_ROOTS_MIGRATION_VERSION) foldPinnedProjectsIntoRoots(decoded);
    if (decoded.version < PLUGIN_KIND_MIGRATION_VERSION) renameLegacyPluginKinds(decoded);
    if (decoded.version < PLUGIN_SETTINGS_MIGRATION_VERSION) moveRundeckSettings(decoded);
    if (decoded.version < ONE_BRUNO_SESSION_MIGRATION_VERSION) mergeBrunoSessions(decoded);
    if (decoded.version < AWS_PLUGIN_MIGRATION_VERSION) moveAwsIntoItsPlugin(decoded);
    if (decoded.version < BRUNO_PLUGIN_MIGRATION_VERSION) moveBrunoIntoItsPlugin(decoded);
    if (decoded.version < RUNDECK_GROUPS_MIGRATION_VERSION) reshapeRundeckSettings(decoded);
    if (decoded.version < DESK_MIGRATION_VERSION) moveBrowserPanesOntoDesks(decoded);
    if (decoded.version < GITHUB_IN_GIT_PANE_MIGRATION_VERSION) closeGithubSessions(decoded);
    if (decoded.version < CORE_SESSION_MIGRATION_VERSION) forgetCoreSessions(decoded);
    if (decoded.version < SSH_LOOP_MIGRATION_VERSION) upgradeSshStartups(decoded);
    if (decoded.version < PANE_TITLE_MIGRATION_VERSION) capitalisePaneTitles(decoded);
    if (decoded.version < TAB_RAIL_MIGRATION_VERSION) renameAgentRailPrefs(decoded);

    const projects = new Map((workspace?.projects ?? []).map((project) => [project.id, project]));
    const sessions: Record<string, Session> = {};
    for (const row of decoded.sessions) {
        if (isRecord(row) && row.kind === "project" && typeof row.host !== "string") {
            const project = typeof row.id === "string" ? projects.get(row.id) : undefined;
            if (project) sessions[project.id] ??= projectSession(project, typeof row.activeWindowId === "string" ? row.activeWindowId : "");
            continue;
        }
        const session = toSession(row);
        if (session && !sessions[session.id]) sessions[session.id] = session;
    }
    for (const project of projects.values()) sessions[project.id] ??= projectSession(project, "");
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
            const restored: Window = { ...row, activePaneId: ids.panes.includes(row.activePaneId) ? row.activePaneId : ids.panes[0] };
            if (sessions[sid].kind === "project") delete restored.fixed;
            if (sessions[sid].kind === "project" && restored.role === "term") restored.name = "Terminal";
            windows[row.id] = restored;
            windowsBySession[sid].push(row.id);
        }
    }
    const cur = getState();
    const prefs = readPrefs(isRecord(decoded.prefs) ? decoded.prefs : {}, cur);
    const providerProfiles = cur.providerProfiles;
    const shared = workspace ? sharedFromCore(workspace) : null;
    const placed = new Set(Object.values(windows).flatMap((win) => (win.role === "agent" ? (agentPaneId(win) ?? []) : [])));
    const claimedResumeIds = new Set<string>();
    if (prefs.restoreAgentTabs) {
        for (const record of workspace?.agents ?? []) {
            const saved = agentFromCore(record);
            if (!saved || agents[saved.id] || sessions[record.project]?.kind !== "project") continue;
            if (saved.resumeId) {
                const claim = chatKey(saved.type, saved.resumeId);
                if (claimedResumeIds.has(claim)) continue;
                claimedResumeIds.add(claim);
            }
            const executablePath = agentExecutablePath({ providerProfiles }, saved);
            if (saved.profileId && !providerProfiles.some((profile) => profile.id === saved.profileId && profile.provider === saved.type)) {
                delete saved.profileId;
            }
            agents[saved.id] = { ...saved, executablePath, launchState: "dormant" };
            if (!placed.has(saved.id)) {
                const win = agentWindow(agents[saved.id], record.cwd);
                windows[win.id] = win;
                windowsBySession[record.project].push(win.id);
            }
        }
    }
    for (const row of Array.isArray(decoded.hostAgents) ? decoded.hostAgents : []) {
        const agent = hostAgent(row);
        if (agent && !agents[agent.id]) agents[agent.id] = agent;
    }
    // An agent window whose record did not come back has nothing to show.
    for (const sid of Object.keys(sessions)) {
        windowsBySession[sid] = windowsBySession[sid].filter((id) => {
            const win = windows[id];
            if (win?.role !== "agent") return true;
            if (sessions[sid].kind === "project" && agentInWindow(agents, win)) return true;
            delete windows[id];
            return false;
        });
        const session = sessions[sid];
        const windowIds = windowsBySession[sid];
        sessions[sid] = { ...session, activeWindowId: windowIds.includes(session.activeWindowId) ? session.activeWindowId : (windowIds[0] ?? "") };
    }

    const editorViews: Record<string, EditorPaneView> = {};
    const deskPanes: Record<string, string> = {};
    const deskRestores: Record<string, DeskView> = {};
    const desks: Record<string, Desk> = {};
    const rawItemStates = isRecord(decoded.itemStates) ? decoded.itemStates : {};
    for (const pane of Object.values(windows).flatMap((window) => collectPanes(window.root))) {
        const itemId = pane.id;
        if (pane.kind === "editor") {
            const view = decodeItemState({ id: itemId, kind: "editor" }, rawItemStates[itemId]);
            if (view) editorViews[itemId] = view;
        }
        if (pane.kind === "desk") {
            const view = decodeItemState({ id: itemId, kind: "desk" }, rawItemStates[itemId]);
            // Without its agent the pane has nothing to be, and the pane
            // itself takes the empty leaf back out of the layout.
            if (!view || !agents[view.agentId]) continue;
            deskPanes[itemId] = view.agentId;
            deskRestores[itemId] = view;
            editorViews[deskEditorId(view.agentId)] = { openTabs: view.files, activePath: view.files[0] ?? null };
            const simulators = view.simulator ? [{ id: newId("desk-simulator"), udid: view.simulator.udid, deviceName: view.simulator.name }] : [];
            const simulatorKeys = simulators.map((simulator) => simulatorKey(simulator.id));
            desks[view.agentId] = {
                order: [...view.files.map(fileKey), ...simulatorKeys],
                active: view.tabs.length > 0 ? BROWSER_ACTIVE : view.files[0] ? fileKey(view.files[0]) : (simulatorKeys[0] ?? null),
                terminals: [],
                simulators,
                reveal: null,
            };
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
        agentStatus: {},
        activeSessionId,
        chosenHost: restoredHostChoice(decoded.chosenHost, sessions, activeSessionId),
        recent: Array.isArray(decoded.recent) ? decoded.recent.filter(isRecent) : [],
        editorViews,
        deskPanes,
        deskRestores,
        desks,
        browserStrips: {},
        ...prefs,
        ...shared,
        activeSpaceId: shared?.spaces.some((space) => space.id === prefs.activeSpaceId) ? prefs.activeSpaceId : null,
        disabledPlugins: [...new Set((workspace?.disabledPlugins ?? []).filter(isPluginId))],
    });
    pruneOnDemandWindows();
    registerCustomThemes(getState().customThemes);
    // The disk still holds `raw`, so the first save after a migration rewrites it in the current form.
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
