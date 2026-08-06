import { invoke } from "@tauri-apps/api/core";
import { sshStartup } from "../terminal/sshStartup";
import { isBuiltinTheme, isTheme } from "../themes";
import { normaliseKeybindingOverrides } from "../keybindings";
import type { CommandContext, CustomCommand, CustomCommandPlacement } from "../commands/registry";
import { registerCustomThemes } from "../themes/bus";
import { ensureSearchWindow, normalisePinnedProjects, normaliseProjectRoots } from "./commands";
import { agentStartup } from "./commands";
import { getState, setState, useStore, type StoreState } from "./store";
import { errMessage, notify } from "./toast";
import { validatePersistedLayout } from "./persistValidation";
import type {
    Agent,
    AgentType,
    EditorPaneView,
    PersistedAgent,
    PersistedPrefs,
    PersistedSession,
    PersistedSnapshot,
    RecentEntry,
    Session,
    Window,
    WindowRole,
} from "./types";

function deriveRole(w: Window): WindowRole {
    if (WINDOW_ROLES.has(w.role)) return w.role;
    if (w.name === "files") return "files";
    if (w.name === "git") return "git";
    if (w.name === "aws") return "aws";
    if (w.name === "rundeck") return "rundeck";
    if (w.name === "bruno") return "bruno";
    if (w.name === "term" || /^\d+$/.test(w.name)) return "term";
    return "named";
}

const VERSION = 6;
const MIN_SUPPORTED_VERSION = 3;
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
    "agentsBySession",
    "activeSessionId",
    "recent",
    "editorViews",
    "pinnedProjects",
    "projectRoots",
    "brunoWorkspaces",
    "themeId",
    "themeMode",
    "systemLightThemeId",
    "systemDarkThemeId",
    "customThemes",
    "windowOpacity",
    "windowBlur",
    "cloudBrowser",
    "cloudBrowserShortcut",
    "keybindingOverrides",
    "awsProfile",
    "awsService",
    "leftRailOpen",
    "rightRailOpen",
    "zenMode",
    "rundeck",
    "restoreAgentTabs",
    "autoResumeAgents",
    "notificationPreferences",
    "railDensity",
    "onboardingComplete",
    "lastSeenVersion",
    "customCommands",
    "updateChannel",
    "lastReleaseNotes",
    "recentCommandKeys",
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
    return {
        projectRoots: s.projectRoots,
        pinnedProjects: s.pinnedProjects,
        brunoWorkspaces: s.brunoWorkspaces,
        themeId: s.themeId,
        themeMode: s.themeMode,
        systemLightThemeId: s.systemLightThemeId,
        systemDarkThemeId: s.systemDarkThemeId,
        customThemes: s.customThemes,
        windowOpacity: s.windowOpacity,
        windowBlur: s.windowBlur,
        cloudBrowser: s.cloudBrowser,
        cloudBrowserShortcut: s.cloudBrowserShortcut,
        keybindingOverrides: s.keybindingOverrides,
        awsProfile: s.awsProfile,
        awsService: s.awsService,
        leftRailOpen: s.leftRailOpen,
        rightRailOpen: s.rightRailOpen,
        zenMode: s.zenMode,
        rundeck: s.rundeck,
        restoreAgentTabs: s.restoreAgentTabs,
        autoResumeAgents: s.autoResumeAgents,
        notificationPreferences: s.notificationPreferences,
        railDensity: s.railDensity,
        onboardingComplete: s.onboardingComplete,
        lastSeenVersion: s.lastSeenVersion,
        customCommands: s.customCommands,
        updateChannel: s.updateChannel,
        lastReleaseNotes: s.lastReleaseNotes,
        recentCommandKeys: s.recentCommandKeys,
    };
}

/** Union of the persisted registry with any currently-open Bruno collection paths, deduped, most-recent-first. */
function mergeBrunoWorkspaces(saved: string[] | undefined, sessions: Session[]): string[] {
    const open = sessions.filter((s) => s.kind === "bruno").map((s) => s.bruno?.collectionPath);
    const out: string[] = [];
    for (const p of [...(saved ?? []), ...open]) if (typeof p === "string" && p && !out.includes(p)) out.push(p);
    return out;
}

const SESSION_KINDS = new Set<Session["kind"]>(["project", "command", "ssh", "aws", "rundeck", "bruno"]);
const WINDOW_ROLES = new Set<WindowRole>(["term", "files", "git", "search", "aws", "rundeck", "bruno", "ssh-config", "named"]);
const AWS_SERVICES = new Set<StoreState["awsService"]>(["ecs", "ec2", "lambda", "sqs", "billing", "s3"]);

function isRecord(value: unknown): value is Record<string, unknown> {
    return !!value && typeof value === "object" && !Array.isArray(value);
}

function isThemeId(value: string, customThemes: unknown): boolean {
    if (isBuiltinTheme(value)) return true;
    return Array.isArray(customThemes) && customThemes.some((theme) => isTheme(theme) && theme.id === value);
}

function normaliseNotificationPreferences(
    value: unknown,
    fallback: StoreState["notificationPreferences"],
    enableByDefault = false,
): StoreState["notificationPreferences"] {
    if (!isRecord(value)) return fallback;
    const time = (candidate: unknown, current: string) =>
        typeof candidate === "string" && /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(candidate) ? candidate : current;
    const mutedAgentTypes = Array.isArray(value.mutedAgentTypes)
        ? value.mutedAgentTypes.filter((v): v is AgentType => AGENT_TYPES.has(v as AgentType))
        : fallback.mutedAgentTypes;
    return {
        // v5 briefly shipped agent alerts disabled by default, making the
        // feature completely silent until users found the setting. Migrate
        // that development schema once; v6+ preserves an explicit opt-out.
        enabled: enableByDefault ? true : typeof value.enabled === "boolean" ? value.enabled : fallback.enabled,
        onlyWhenUnfocused: typeof value.onlyWhenUnfocused === "boolean" ? value.onlyWhenUnfocused : fallback.onlyWhenUnfocused,
        sounds: typeof value.sounds === "boolean" ? value.sounds : fallback.sounds,
        soundStyle: value.soundStyle === "soft" || value.soundStyle === "bright" ? value.soundStyle : fallback.soundStyle,
        delayMs:
            typeof value.delayMs === "number" && Number.isFinite(value.delayMs) ? Math.min(10_000, Math.max(0, value.delayMs)) : fallback.delayMs,
        quietHoursEnabled: typeof value.quietHoursEnabled === "boolean" ? value.quietHoursEnabled : fallback.quietHoursEnabled,
        quietHoursStart: time(value.quietHoursStart, fallback.quietHoursStart),
        quietHoursEnd: time(value.quietHoursEnd, fallback.quietHoursEnd),
        mutedAgentTypes,
    };
}

const COMMAND_CONTEXTS = new Set<CommandContext>(["project", "command", "ssh", "aws", "rundeck", "bruno"]);
const COMMAND_PLACEMENTS = new Set<CustomCommandPlacement>(["background", "terminal", "split", "popup", "replace"]);

function normaliseCustomCommands(value: unknown): CustomCommand[] {
    if (!Array.isArray(value)) return [];
    const seen = new Set<string>();
    const commands: CustomCommand[] = [];
    for (const row of value) {
        if (!isRecord(row) || typeof row.id !== "string" || !row.id || seen.has(row.id)) continue;
        if (typeof row.title !== "string" || !row.title.trim() || typeof row.command !== "string" || !row.command.trim()) continue;
        if (!COMMAND_PLACEMENTS.has(row.placement as CustomCommandPlacement)) continue;
        const contexts = Array.isArray(row.contexts)
            ? row.contexts.filter((v): v is CommandContext => COMMAND_CONTEXTS.has(v as CommandContext))
            : [];
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

function isStringRecord(value: unknown): value is Record<string, string> {
    return isRecord(value) && Object.values(value).every((item) => typeof item === "string");
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
        !SESSION_KINDS.has(value.kind as Session["kind"]) ||
        typeof value.cwd !== "string" ||
        typeof value.pinned !== "boolean" ||
        typeof value.activeWindowId !== "string" ||
        !(value.activeAgentId === null || typeof value.activeAgentId === "string") ||
        !(value.view === "windows" || value.view === "agent")
    ) {
        return null;
    }
    const deploy =
        isRecord(value.deploy) &&
        typeof value.deploy.project === "string" &&
        (value.deploy.folder === null || typeof value.deploy.folder === "string")
            ? { project: value.deploy.project, folder: value.deploy.folder }
            : null;
    const session: Session = {
        id: value.id,
        name: value.name,
        kind: value.kind as Session["kind"],
        cwd: value.cwd,
        deploy,
        pinned: value.pinned,
        activeWindowId: value.activeWindowId,
        activeAgentId: value.activeAgentId,
        view: value.view,
    };
    if (session.kind === "bruno") {
        const bruno = isRecord(value.bruno) ? value.bruno : {};
        session.bruno = {
            collectionPath: typeof bruno.collectionPath === "string" ? bruno.collectionPath : session.cwd,
            selectedEnvs: isStringRecord(bruno.selectedEnvs) ? bruno.selectedEnvs : {},
            // Older snapshots may contain credentials. Never restore them into runtime state.
            secretVars: {},
            drafts: {},
        };
    } else {
        delete session.bruno;
    }
    return session;
}

function isEditorView(value: unknown): value is EditorPaneView {
    return (
        isRecord(value) &&
        Array.isArray(value.openTabs) &&
        value.openTabs.every((p) => typeof p === "string") &&
        (value.activePath === null || typeof value.activePath === "string") &&
        typeof value.treeWidth === "number" &&
        Number.isFinite(value.treeWidth)
    );
}

function isRecent(value: unknown): value is RecentEntry {
    return isRecord(value) && SESSION_KINDS.has(value.kind as Session["kind"]) && typeof value.name === "string" && typeof value.cwd === "string";
}

const AGENT_TYPES = new Set<AgentType>(["claude", "codex", "hermes", "pi", "opencode"]);

function toPersistedAgent(value: unknown): PersistedAgent | null {
    if (!isRecord(value) || typeof value.id !== "string" || !value.id || !AGENT_TYPES.has(value.type as AgentType)) return null;
    if (typeof value.title !== "string" || !value.title.trim() || typeof value.resumeId !== "string" || !value.resumeId.trim()) return null;
    const agent: PersistedAgent = { id: value.id, type: value.type as AgentType, title: value.title.slice(0, 200), resumeId: value.resumeId };
    if (typeof value.skipPermissions === "boolean") agent.skipPermissions = value.skipPermissions;
    return agent;
}

function persistedSession(sess: Session, activeAgentId: string | null, view: Session["view"]): PersistedSession {
    const { bruno, ...base } = sess;
    if (sess.kind !== "bruno" || !bruno) return { ...base, activeAgentId, view };
    return {
        ...base,
        activeAgentId,
        view,
        bruno: { collectionPath: bruno.collectionPath, selectedEnvs: bruno.selectedEnvs },
    };
}

function snapshot(): string {
    const s = getState();
    const sessions = s.sessionOrder
        .map((id) => s.sessions[id])
        .filter(Boolean)
        .map((sess) => {
            const safeAgentIds = (s.agentsBySession[sess.id] ?? []).filter((id) => !!s.agents[id]?.resumeId);
            const activeAgentId = sess.activeAgentId && safeAgentIds.includes(sess.activeAgentId) ? sess.activeAgentId : null;
            return persistedSession(sess, activeAgentId, sess.view === "agent" && activeAgentId ? "agent" : "windows");
        });
    const windowsBySession: Record<string, Window[]> = {};
    const agentsBySession: Record<string, PersistedAgent[]> = {};
    for (const sess of sessions) {
        windowsBySession[sess.id] = (s.windowsBySession[sess.id] ?? []).map((id) => s.windows[id]).filter(Boolean);
        agentsBySession[sess.id] = (s.agentsBySession[sess.id] ?? [])
            .map((id) => s.agents[id])
            .filter((agent): agent is Agent => !!agent?.resumeId)
            .map(({ id, type, title, resumeId, skipPermissions }) => ({
                id,
                type,
                title,
                resumeId,
                ...(typeof skipPermissions === "boolean" ? { skipPermissions } : {}),
            }));
    }
    const snap: PersistedSnapshot = {
        version: VERSION,
        sessions,
        windowsBySession,
        agentsBySession,
        sessionOrder: sessions.map((s) => s.id),
        activeSessionId: s.activeSessionId,
        recent: s.recent,
        prefs: packPrefs(s),
        editorViews: s.editorViews,
    };
    // Defense in depth: these runtime-only Bruno fields must never reach disk,
    // even if a malformed record introduced them outside the typed session shape.
    return JSON.stringify(snap, (key, value) => (key === "secretVars" || key === "drafts" ? undefined : value));
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

export function applyHydrate(raw: string): void {
    if (!raw) return;
    let decoded: unknown;
    try {
        decoded = JSON.parse(raw);
    } catch {
        return;
    }
    if (!isRecord(decoded) || typeof decoded.version !== "number" || decoded.version < MIN_SUPPORTED_VERSION || decoded.version > VERSION) return;
    if (!Array.isArray(decoded.sessions)) return;

    const sessions: Record<string, Session> = {};
    for (const row of decoded.sessions) {
        const session = toSession(row);
        if (session && !sessions[session.id]) sessions[session.id] = session;
    }
    if (Object.keys(sessions).length === 0) return;

    const windows: Record<string, Window> = {};
    const agents: Record<string, Agent> = {};
    const windowsBySession: Record<string, string[]> = {};
    const agentsBySession: Record<string, string[]> = {};
    const rawWindows = isRecord(decoded.windowsBySession) ? decoded.windowsBySession : {};
    const usedLayoutIds = new Set<string>();
    for (const sid of Object.keys(sessions)) {
        const rows = Array.isArray(rawWindows[sid]) ? rawWindows[sid] : [];
        windowsBySession[sid] = [];
        let projectTerminalNumber = 0;
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
            if (sessions[sid].kind === "project" && restored.role === "term") {
                restored.name = String(++projectTerminalNumber);
                delete restored.fixed;
            }
            windows[row.id] = restored;
            windowsBySession[sid].push(row.id);
        }
        agentsBySession[sid] = [];
    }
    const prefs = isRecord(decoded.prefs) ? decoded.prefs : {};
    const restoreAgentTabs = typeof prefs.restoreAgentTabs === "boolean" ? prefs.restoreAgentTabs : true;
    const autoResumeAgents = typeof prefs.autoResumeAgents === "boolean" ? prefs.autoResumeAgents : false;
    const rawAgents = isRecord(decoded.agentsBySession) ? decoded.agentsBySession : {};
    const claimedResumeIds = new Set<string>();
    if (restoreAgentTabs) {
        for (const sid of Object.keys(sessions)) {
            if (sessions[sid].kind !== "project") continue;
            const rows = Array.isArray(rawAgents[sid]) ? rawAgents[sid] : [];
            for (const row of rows) {
                const saved = toPersistedAgent(row);
                if (!saved || agents[saved.id]) continue;
                const claim = `${saved.type}\0${saved.resumeId}`;
                if (claimedResumeIds.has(claim)) continue;
                claimedResumeIds.add(claim);
                agents[saved.id] = {
                    ...saved,
                    startup: agentStartup(saved.type, saved.resumeId, saved.skipPermissions ?? false),
                    launchState: autoResumeAgents ? "live" : "dormant",
                };
                agentsBySession[sid].push(saved.id);
            }
        }
    }
    // Restored agent tabs are inert by default. Startup is rebuilt from the
    // trusted agent type/resume id pair above and never read from disk.
    for (const sid of Object.keys(sessions)) {
        const session = sessions[sid];
        const agentIds = agentsBySession[sid];
        const windowIds = windowsBySession[sid];
        const savedActiveAgentId = session.activeAgentId && agentIds.includes(session.activeAgentId) ? session.activeAgentId : null;
        const activeAgentId = savedActiveAgentId ?? (session.view === "agent" ? (agentIds[0] ?? null) : null);
        sessions[sid] = {
            ...session,
            activeWindowId: windowIds.includes(session.activeWindowId) ? session.activeWindowId : (windowIds[0] ?? ""),
            activeAgentId,
            view: session.view === "agent" && activeAgentId ? "agent" : "windows",
        };
    }

    const validPaneIds = new Set<string>();
    for (const w of Object.values(windows)) {
        const walk = (n: Window["root"]): void => {
            if (n.type === "pane") validPaneIds.add(n.id);
            else n.children.forEach(walk);
        };
        walk(w.root);
    }
    const editorViews: Record<string, EditorPaneView> = {};
    const rawEditorViews = isRecord(decoded.editorViews) ? decoded.editorViews : {};
    for (const [pid, value] of Object.entries(rawEditorViews)) if (validPaneIds.has(pid) && isEditorView(value)) editorViews[pid] = value;

    const requestedOrder = Array.isArray(decoded.sessionOrder) ? decoded.sessionOrder.filter((id): id is string => typeof id === "string") : [];
    const sessionOrder = [...new Set(requestedOrder.filter((id) => sessions[id]))];
    for (const sid of Object.keys(sessions)) if (!sessionOrder.includes(sid)) sessionOrder.push(sid);
    const requestedActive = typeof decoded.activeSessionId === "string" ? decoded.activeSessionId : "";
    const activeSessionId = sessions[requestedActive] ? requestedActive : sessionOrder[0];
    const cur = getState();
    const rundeck = isRecord(prefs.rundeck) ? prefs.rundeck : {};
    const prodEnvs = Array.isArray(rundeck.prodEnvs) ? rundeck.prodEnvs.filter((v): v is string => typeof v === "string") : cur.rundeck.prodEnvs;

    setState({
        sessions,
        windows,
        agents,
        sessionOrder,
        windowsBySession,
        agentsBySession,
        agentActivity: {},
        activeSessionId,
        recent: Array.isArray(decoded.recent) ? decoded.recent.filter(isRecent) : [],
        editorViews,
        pinnedProjects: normalisePinnedProjects(Array.isArray(prefs.pinnedProjects) ? prefs.pinnedProjects : []),
        projectRoots: Array.isArray(prefs.projectRoots) ? normaliseProjectRoots(prefs.projectRoots) : cur.projectRoots,
        brunoWorkspaces: mergeBrunoWorkspaces(
            Array.isArray(prefs.brunoWorkspaces) ? prefs.brunoWorkspaces.filter((v): v is string => typeof v === "string") : undefined,
            Object.values(sessions),
        ),
        themeId: typeof prefs.themeId === "string" ? prefs.themeId : cur.themeId,
        themeMode: prefs.themeMode === "system" || prefs.themeMode === "manual" ? prefs.themeMode : cur.themeMode,
        systemLightThemeId:
            typeof prefs.systemLightThemeId === "string" && isThemeId(prefs.systemLightThemeId, prefs.customThemes)
                ? prefs.systemLightThemeId
                : cur.systemLightThemeId,
        systemDarkThemeId:
            typeof prefs.systemDarkThemeId === "string" && isThemeId(prefs.systemDarkThemeId, prefs.customThemes)
                ? prefs.systemDarkThemeId
                : cur.systemDarkThemeId,
        customThemes: Array.isArray(prefs.customThemes) ? prefs.customThemes.filter(isTheme) : cur.customThemes,
        windowOpacity: typeof prefs.windowOpacity === "number" && Number.isFinite(prefs.windowOpacity) ? prefs.windowOpacity : cur.windowOpacity,
        windowBlur: typeof prefs.windowBlur === "number" && Number.isFinite(prefs.windowBlur) ? prefs.windowBlur : cur.windowBlur,
        cloudBrowser: typeof prefs.cloudBrowser === "string" ? prefs.cloudBrowser : cur.cloudBrowser,
        cloudBrowserShortcut: typeof prefs.cloudBrowserShortcut === "string" ? prefs.cloudBrowserShortcut : cur.cloudBrowserShortcut,
        keybindingOverrides: normaliseKeybindingOverrides(prefs.keybindingOverrides),
        awsProfile: prefs.awsProfile === null || typeof prefs.awsProfile === "string" ? prefs.awsProfile : cur.awsProfile,
        awsService: AWS_SERVICES.has(prefs.awsService as StoreState["awsService"]) ? (prefs.awsService as StoreState["awsService"]) : cur.awsService,
        leftRailOpen: typeof prefs.leftRailOpen === "boolean" ? prefs.leftRailOpen : cur.leftRailOpen,
        rightRailOpen: typeof prefs.rightRailOpen === "boolean" ? prefs.rightRailOpen : cur.rightRailOpen,
        zenMode: typeof prefs.zenMode === "boolean" ? prefs.zenMode : cur.zenMode,
        rundeck: {
            activeProject: typeof rundeck.activeProject === "string" ? rundeck.activeProject : "",
            activeEnvFolder: rundeck.activeEnvFolder === null || typeof rundeck.activeEnvFolder === "string" ? rundeck.activeEnvFolder : null,
            prodEnvs,
        },
        restoreAgentTabs,
        autoResumeAgents,
        notificationPreferences: normaliseNotificationPreferences(
            prefs.notificationPreferences,
            cur.notificationPreferences,
            decoded.version < VERSION,
        ),
        railDensity: prefs.railDensity === "compact" || prefs.railDensity === "comfortable" ? prefs.railDensity : cur.railDensity,
        onboardingComplete:
            typeof prefs.onboardingComplete === "boolean" ? prefs.onboardingComplete : decoded.version < VERSION ? true : cur.onboardingComplete,
        lastSeenVersion: typeof prefs.lastSeenVersion === "string" ? prefs.lastSeenVersion : cur.lastSeenVersion,
        customCommands: normaliseCustomCommands(prefs.customCommands),
        updateChannel: prefs.updateChannel === "preview" || prefs.updateChannel === "stable" ? prefs.updateChannel : cur.updateChannel,
        lastReleaseNotes:
            isRecord(prefs.lastReleaseNotes) && typeof prefs.lastReleaseNotes.version === "string"
                ? {
                      version: prefs.lastReleaseNotes.version,
                      notes: typeof prefs.lastReleaseNotes.notes === "string" ? prefs.lastReleaseNotes.notes : null,
                      date: typeof prefs.lastReleaseNotes.date === "string" ? prefs.lastReleaseNotes.date : null,
                  }
                : null,
        recentCommandKeys: Array.isArray(prefs.recentCommandKeys)
            ? prefs.recentCommandKeys.filter((value): value is string => typeof value === "string").slice(0, 20)
            : [],
    });
    ensureSearchWindow();
    registerCustomThemes(getState().customThemes);
    // Preserve the actual disk payload as the saved marker. The subscription
    // rewrites migrations and sanitized legacy credentials in canonical v6 form.
    lastSaved = raw;
    lastSlices = takeSlices(getState());
}

export function canFlushPersist(): boolean {
    return persistenceReady;
}

export function subscribePersist(): () => void {
    persistenceReady = true;
    queueSnapshot(snapshot());
    void startSaveLoop();
    const unsubscribe = useStore.subscribe(() => {
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
