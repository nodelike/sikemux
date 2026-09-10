import { invokeCommand as invoke } from "../api/invoke";
import { sshStartup } from "../terminal/sshStartup";
import { isBuiltinTheme, isTheme } from "../themes";
import { normaliseKeybindingOverrides } from "../keybindings";
import type { CommandContext, CustomCommand, CustomCommandPlacement } from "../commands/registry";
import { registerCustomThemes } from "../themes/bus";
import { normalizePermissionMode } from "../agentLaunch";
import { mergePinnedIntoRoots, normaliseProjectRoots, pruneOnDemandWindows } from "./commands";
import { agentDirectCommand, agentStartup } from "./commands";
import { getState, setState, useStore, type StoreState } from "./store";
import { errMessage, notify } from "./toast";
import { validatePersistedLayout } from "./persistValidation";
import { createWorkbenchItemRef, workbenchItemRegistry, workbenchItemRefFromPane } from "../workbench/registry";
import type {
    Agent,
    AgentPermissionMode,
    AgentProvider,
    AgentType,
    EditorPaneView,
    PersistedAgent,
    PersistedPrefs,
    PersistedSession,
    PersistedSnapshot,
    ProviderProfile,
    ProviderProfileSelection,
    RailTab,
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

const VERSION = 7;
const MIN_SUPPORTED_VERSION = 3;
const ONBOARDING_MIGRATION_VERSION = 6;
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
    "projectRoots",
    "brunoWorkspaces",
    "themeId",
    "themeMode",
    "systemLightThemeId",
    "systemDarkThemeId",
    "customThemes",
    "uiTextScale",
    "windowOpacity",
    "windowBlur",
    "cloudBrowser",
    "cloudBrowserShortcut",
    "keybindingOverrides",
    "awsProfile",
    "awsService",
    "sideRailOpen",
    "workspaceRailOpen",
    "railTab",
    "zenMode",
    "rundeck",
    "restoreAgentTabs",
    "railDensity",
    "onboardingComplete",
    "lastSeenVersion",
    "customCommands",
    "updateChannel",
    "lastReleaseNotes",
    "recentCommandKeys",
    "providerProfiles",
    "selectedProviderProfileIds",
    "defaultAgentPermissionMode",
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
        brunoWorkspaces: s.brunoWorkspaces,
        themeId: s.themeId,
        themeMode: s.themeMode,
        systemLightThemeId: s.systemLightThemeId,
        systemDarkThemeId: s.systemDarkThemeId,
        customThemes: s.customThemes,
        uiTextScale: s.uiTextScale,
        windowOpacity: s.windowOpacity,
        windowBlur: s.windowBlur,
        cloudBrowser: s.cloudBrowser,
        cloudBrowserShortcut: s.cloudBrowserShortcut,
        keybindingOverrides: s.keybindingOverrides,
        awsProfile: s.awsProfile,
        awsService: s.awsService,
        sideRailOpen: s.sideRailOpen,
        workspaceRailOpen: s.workspaceRailOpen,
        railTab: s.railTab,
        zenMode: s.zenMode,
        rundeck: s.rundeck,
        restoreAgentTabs: s.restoreAgentTabs,
        railDensity: s.railDensity,
        onboardingComplete: s.onboardingComplete,
        lastSeenVersion: s.lastSeenVersion,
        customCommands: s.customCommands,
        updateChannel: s.updateChannel,
        lastReleaseNotes: s.lastReleaseNotes,
        recentCommandKeys: s.recentCommandKeys,
        providerProfiles,
        selectedProviderProfileIds: normaliseProviderProfileSelection(s.selectedProviderProfileIds, providerProfiles, {}),
        defaultAgentPermissionMode: s.defaultAgentPermissionMode === "bypass" ? "bypass" : "workspace-write",
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
const WINDOW_ROLES = new Set<WindowRole>(["term", "files", "git", "diff", "search", "aws", "rundeck", "bruno", "ssh-config", "named"]);
const RAIL_TABS = new Set<RailTab>(["agents", "files", "changes", "search"]);
const AWS_SERVICES = new Set<StoreState["awsService"]>(["ecs", "ec2", "lambda", "sqs", "billing", "s3"]);

function isRecord(value: unknown): value is Record<string, unknown> {
    return !!value && typeof value === "object" && !Array.isArray(value);
}

function isThemeId(value: string, customThemes: unknown): boolean {
    if (isBuiltinTheme(value)) return true;
    return Array.isArray(customThemes) && customThemes.some((theme) => isTheme(theme) && theme.id === value);
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

function isRecent(value: unknown): value is RecentEntry {
    return isRecord(value) && SESSION_KINDS.has(value.kind as Session["kind"]) && typeof value.name === "string" && typeof value.cwd === "string";
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

const AGENT_EFFORTS = new Set(["off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]);
function toPersistedAgent(value: unknown): PersistedAgent | null {
    if (!isRecord(value) || typeof value.id !== "string" || !value.id || !AGENT_TYPES.has(value.type as AgentType)) return null;
    if (typeof value.title !== "string" || !value.title.trim() || typeof value.resumeId !== "string" || !value.resumeId.trim()) return null;
    const agent: PersistedAgent = { id: value.id, type: value.type as AgentType, title: value.title.slice(0, 200), resumeId: value.resumeId };
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
            const durableWindowIds = (s.windowsBySession[sess.id] ?? []).filter((id) => !!s.windows[id] && !s.windows[id].transient);
            const activeWindowId = durableWindowIds.includes(sess.activeWindowId) ? sess.activeWindowId : (durableWindowIds[0] ?? "");
            return persistedSession({ ...sess, activeWindowId }, activeAgentId, sess.view === "agent" && activeAgentId ? "agent" : "windows");
        });
    const windowsBySession: Record<string, Window[]> = {};
    const agentsBySession: Record<string, PersistedAgent[]> = {};
    const itemStates: PersistedSnapshot["itemStates"] = {};
    for (const sess of sessions) {
        windowsBySession[sess.id] = (s.windowsBySession[sess.id] ?? [])
            .map((id) => s.windows[id])
            .filter((window): window is Window => !!window && !window.transient);
        for (const window of windowsBySession[sess.id]) {
            const pending = [window.root];
            while (pending.length > 0) {
                const node = pending.pop()!;
                if (node.type === "split") {
                    pending.push(...node.children);
                    continue;
                }
                if (node.kind !== "editor") continue;
                const ref = createWorkbenchItemRef(node.id, "editor");
                const state = s.editorViews[node.id] ?? { openTabs: [], activePath: null };
                try {
                    itemStates[node.id] = workbenchItemRegistry.encodePersisted(ref, state);
                } catch {
                    // One malformed runtime item must not block the durable
                    // topology and every other valid item from being saved.
                }
            }
        }
        agentsBySession[sess.id] = (s.agentsBySession[sess.id] ?? [])
            .map((id) => s.agents[id])
            .filter((agent): agent is Agent => !!agent?.resumeId)
            .map((agent) => {
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
                };
            });
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
        itemStates,
    };
    // Defense in depth: these runtime-only Bruno fields must never reach disk,
    // even if a malformed record introduced them outside the typed session shape.
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

    const sessions: Record<string, Session> = {};
    for (const row of decoded.sessions) {
        const session = toSession(row);
        if (session && !sessions[session.id]) sessions[session.id] = session;
    }
    if (Object.keys(sessions).length === 0) return "invalid";

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
    const cur = getState();
    const providerProfiles = normaliseProviderProfiles(prefs.providerProfiles, cur.providerProfiles);
    const restoreAgentTabs = typeof prefs.restoreAgentTabs === "boolean" ? prefs.restoreAgentTabs : true;
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
                agents[saved.id] = {
                    ...saved,
                    permissionMode,
                    executablePath,
                    startup: agentStartup(saved.type, saved.resumeId, permissionMode, executablePath, launchOptions),
                    directCommand: agentDirectCommand(saved.type, saved.resumeId, permissionMode, executablePath, launchOptions),
                    launchState: "dormant",
                };
                agentsBySession[sid].push(saved.id);
            }
        }
    }
    // Startup is rebuilt from the
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

    const panesById = new Map<string, ReturnType<typeof workbenchItemRefFromPane>>();
    for (const w of Object.values(windows)) {
        const walk = (n: Window["root"]): void => {
            if (n.type === "pane") panesById.set(n.id, workbenchItemRefFromPane(n));
            else n.children.forEach(walk);
        };
        walk(w.root);
    }
    const editorViews: Record<string, EditorPaneView> = {};
    if (decoded.version >= 7) {
        const rawItemStates = isRecord(decoded.itemStates) ? decoded.itemStates : {};
        for (const [itemId, ref] of panesById) {
            const result = workbenchItemRegistry.decodePersisted(ref, rawItemStates[itemId]);
            if (result.ok && result.ref.kind === "editor") editorViews[itemId] = result.state as EditorPaneView;
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
        // Pinned projects used to be their own list; they are self-indexed
        // roots now, folded in here so existing setups carry over untouched.
        projectRoots: mergePinnedIntoRoots(
            Array.isArray(prefs.projectRoots) ? normaliseProjectRoots(prefs.projectRoots) : cur.projectRoots,
            prefs.pinnedProjects,
        ),
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
        uiTextScale: typeof prefs.uiTextScale === "number" && [1, 1.1, 1.25].includes(prefs.uiTextScale) ? prefs.uiTextScale : cur.uiTextScale,
        windowOpacity: typeof prefs.windowOpacity === "number" && Number.isFinite(prefs.windowOpacity) ? prefs.windowOpacity : cur.windowOpacity,
        windowBlur: typeof prefs.windowBlur === "number" && Number.isFinite(prefs.windowBlur) ? prefs.windowBlur : cur.windowBlur,
        cloudBrowser: typeof prefs.cloudBrowser === "string" ? prefs.cloudBrowser : cur.cloudBrowser,
        cloudBrowserShortcut: typeof prefs.cloudBrowserShortcut === "string" ? prefs.cloudBrowserShortcut : cur.cloudBrowserShortcut,
        keybindingOverrides: normaliseKeybindingOverrides(prefs.keybindingOverrides),
        awsProfile: prefs.awsProfile === null || typeof prefs.awsProfile === "string" ? prefs.awsProfile : cur.awsProfile,
        awsService: AWS_SERVICES.has(prefs.awsService as StoreState["awsService"]) ? (prefs.awsService as StoreState["awsService"]) : cur.awsService,
        sideRailOpen: typeof prefs.sideRailOpen === "boolean" ? prefs.sideRailOpen : cur.sideRailOpen,
        workspaceRailOpen: typeof prefs.workspaceRailOpen === "boolean" ? prefs.workspaceRailOpen : cur.workspaceRailOpen,
        railTab: RAIL_TABS.has(prefs.railTab as RailTab) ? (prefs.railTab as RailTab) : cur.railTab,
        zenMode: typeof prefs.zenMode === "boolean" ? prefs.zenMode : cur.zenMode,
        rundeck: {
            activeProject: typeof rundeck.activeProject === "string" ? rundeck.activeProject : "",
            activeEnvFolder: rundeck.activeEnvFolder === null || typeof rundeck.activeEnvFolder === "string" ? rundeck.activeEnvFolder : null,
            prodEnvs,
        },
        restoreAgentTabs,
        railDensity: prefs.railDensity === "compact" || prefs.railDensity === "comfortable" ? prefs.railDensity : cur.railDensity,
        onboardingComplete:
            typeof prefs.onboardingComplete === "boolean"
                ? prefs.onboardingComplete
                : decoded.version < ONBOARDING_MIGRATION_VERSION
                  ? true
                  : cur.onboardingComplete,
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
        providerProfiles,
        selectedProviderProfileIds: normaliseProviderProfileSelection(
            prefs.selectedProviderProfileIds,
            providerProfiles,
            cur.selectedProviderProfileIds,
        ),
        defaultAgentPermissionMode:
            prefs.defaultAgentPermissionMode === undefined
                ? cur.defaultAgentPermissionMode
                : prefs.defaultAgentPermissionMode === "bypass"
                  ? "bypass"
                  : "workspace-write",
    });
    pruneOnDemandWindows();
    registerCustomThemes(getState().customThemes);
    // Preserve the actual disk payload as the saved marker. The subscription
    // rewrites migrations and sanitized legacy credentials in canonical v7 form.
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
