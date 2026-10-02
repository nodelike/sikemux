import { agentApi, type AgentSession } from "../../api/agents";
import type { AcpChat } from "../../api/acp";
import { profileOfLauncher } from "../../remote/workspace";
import { agentSupportsChat, isAgentType, MAX_AGENT_MODEL_LENGTH, normalizePermissionMode, type ChatAgentType } from "../../agents/agentLaunch";
import { emit } from "../bus";
import { reduceAgentState } from "../agentStatus";
import { invalidate, peekResource } from "../resources";
import { agentSessionsR } from "../resources.defs";
import { getState, mutate, type StoreState } from "../store";
import { notify, reportError, swallow } from "../toast";
import { agentIdsWithLiveSessions } from "../agentLiveSessions";
import { agentSupportsSkipPermissions } from "./agentLogic";
import { agentDirectCommand, agentStartup } from "./agentLaunchCommand";
import { activeAgentId, agentIdsOf, agentWindowId, ownerSessionId } from "../selectors";
import { agentWindow } from "../agentWindow";
import { newId } from "../layout";
import type { Agent, AgentEffort, AgentPermissionMode, AgentType, AgentWorktree, ProviderProfile } from "../types";
import { selectSession } from "./sessions";
import { keepOpenedProjectInView, openProjectSession, projectSessionInBackground, withActiveSession } from "./shared";
import { closeWindowById } from "./tabs";

const FALLBACK_AGENT_TITLE_MAX = 13;
/** Providers list a session under at most this many characters. */
const SESSION_TITLE_MAX = 72;

function profileLaunchOptions(profile: ProviderProfile | undefined, model?: string, effort?: AgentEffort) {
    return {
        model,
        effort,
        configPath: profile?.configPath,
        environmentKeys: profile?.environmentKeys,
    };
}

function usableAgentSessionTitle(row: AgentSession, current: string): string {
    const title = row.title.trim();
    if (!title) return current;
    if (title.length <= FALLBACK_AGENT_TITLE_MAX && row.id.startsWith(title)) return current;
    return title;
}

export function agentSessionMetadataPending(agent: Agent): boolean {
    if (!agent.resumeId) return true;
    const title = agent.title.trim();
    if (!title || title.toLowerCase() === agent.type) return true;
    return title.length <= FALLBACK_AGENT_TITLE_MAX && agent.resumeId.startsWith(title);
}

export function configureEmptyAgent(id: string, type: ChatAgentType, profileId?: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        const activity = d.agentActivity[id];
        if (!agent || activity?.backendState === "working" || activity?.backendState === "blocked") return;
        const profile = profileId ? d.providerProfiles.find((item) => item.id === profileId && item.provider === type) : undefined;
        if (profileId && !profile) return;
        const mode = normalizePermissionMode(type, agent.permissionMode ?? d.defaultAgentPermissionMode);
        agent.type = type;
        agent.profileId = profile?.id;
        agent.title = profile?.name || type;
        agent.executablePath = profile?.executablePath;
        agent.permissionMode = mode;
        agent.skipPermissions = mode === "bypass";
        delete agent.resumeId;
        delete agent.model;
        delete agent.effort;
        delete agent.baselineSessionIds;
        const options = profileLaunchOptions(profile);
        agent.startup = agentStartup(type, undefined, mode, profile?.executablePath, options);
        agent.directCommand = agentDirectCommand(type, undefined, mode, profile?.executablePath, options);
    });
}

export function setAgentModelPreferences(id: string, model: string | undefined, effort: AgentEffort | undefined): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.model = model;
        agent.effort = effort;
        const profile = d.providerProfiles.find((item) => item.id === agent.profileId && item.provider === agent.type);
        const options = profileLaunchOptions(profile, model, effort);
        const executable = profile?.executablePath || agent.executablePath;
        const mode = agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write");
        agent.startup = agentStartup(agent.type, agent.resumeId, mode, executable, options);
        agent.directCommand = agentDirectCommand(agent.type, agent.resumeId, mode, executable, options);
    });
}

export function setAgentPermissionMode(id: string, requestedMode: AgentPermissionMode): void {
    mutate((d) => {
        const currentAgent = d.agents[id];
        const profile = currentAgent?.profileId
            ? d.providerProfiles.find((item) => item.id === currentAgent.profileId && item.provider === currentAgent.type)
            : undefined;
        const a = d.agents[id];
        if (!a) return;
        const next = normalizePermissionMode(a.type, requestedMode);
        const current = a.permissionMode ?? (a.skipPermissions ? "bypass" : normalizePermissionMode(a.type, "workspace-write"));
        if (next === current) return;
        a.permissionMode = next;
        a.skipPermissions = next === "bypass";
        const launchOptions = profileLaunchOptions(profile, a.model, a.effort);
        const executablePath = profile?.executablePath || a.executablePath;
        a.startup = agentStartup(a.type, a.resumeId, next, executablePath, launchOptions);
        a.directCommand = agentDirectCommand(a.type, a.resumeId, next, executablePath, launchOptions);
    });
}

export function toggleAgentSkipPermissions(id: string): void {
    const agent = getState().agents[id];
    if (!agent || !agentSupportsSkipPermissions(agent.type)) return;
    const current = agent.permissionMode ?? (agent.skipPermissions ? "bypass" : normalizePermissionMode(agent.type, "workspace-write"));
    const next = current === "bypass" ? normalizePermissionMode(agent.type, "workspace-write") : "bypass";
    setAgentPermissionMode(id, next);
}

/** ⌥Y — toggle YOLO (skip-permissions) for the active agent, when one is on screen. */
export function toggleActiveAgentSkipPermissions(): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    const id = activeAgentId(st, session);
    if (id) toggleAgentSkipPermissions(id);
}

export interface AddAgentOptions {
    permissionMode?: AgentPermissionMode;
    profileId?: string | null;
    model?: string;
    effort?: AgentEffort;
    baselineSessionIds?: string[];
    cwd?: string;
    /** Pin launches to the project that opened the picker. */
    sessionId?: string;
    detectedExecutablePath?: string;
}

export function addAgent(type: AgentType, resumeId?: string, title?: string, options: AddAgentOptions = {}): boolean {
    if ((options.model?.trim().length ?? 0) > MAX_AGENT_MODEL_LENGTH) return false;
    let attached = false;
    mutate((d) => {
        const session = d.sessions[options.sessionId ?? d.activeSessionId];
        if (!session) return;
        if (session.kind !== "project") return;
        const existing = resumeId
            ? agentIdsOf(d, session.id)
                  .map((id) => d.agents[id])
                  .find((a) => a && a.type === type && a.resumeId === resumeId)
            : undefined;
        const sess = d.sessions[session.id];
        d.zoomedPaneId = null;
        // A successful launch closes the picker and activates the new PTY.
        d.agentPaletteOpen = false;
        if (existing) {
            const winId = agentWindowId(d, existing.id);
            if (winId) sess.activeWindowId = winId;
            attached = true;
            return;
        }
        const permissionMode = normalizePermissionMode(type, options.permissionMode ?? d.defaultAgentPermissionMode);
        const requestedProfileId = options.profileId === undefined ? d.selectedProviderProfileIds[type] : options.profileId;
        const profileId = requestedProfileId
            ? d.providerProfiles.find((profile) => profile.id === requestedProfileId && profile.provider === type)?.id
            : undefined;
        const cwd = options.cwd || session.cwd;
        const model = options.model?.trim() || undefined;
        const profile = profileId ? d.providerProfiles.find((item) => item.id === profileId) : undefined;
        const executablePath = profile?.executablePath || options.detectedExecutablePath;
        const launchOptions = profileLaunchOptions(profile, model, options.effort);
        const agent: Agent = {
            id: newId("agent"),
            type,
            title: title ?? type,
            startup: agentStartup(type, resumeId, permissionMode, executablePath, launchOptions),
            directCommand: agentDirectCommand(type, resumeId, permissionMode, executablePath, launchOptions),
            resumeId,
            createdAt: Date.now(),
            permissionMode,
            profileId,
            executablePath,
            cwd,
            model,
            effort: options.effort,
            ...(permissionMode === "bypass" ? { skipPermissions: true } : {}),
            launchState: "live",
        };
        // Fresh agents (no resumeId) record the sessions that already exist so
        // reconciliation never adopts the session you were just in. The rail
        // keeps this list warm; on a cold cache we fall back to an mtime check.
        if (!resumeId) {
            const known = options.baselineSessionIds ?? peekResource(agentSessionsR, type, cwd, profile?.configPath)?.map((row) => row.id);
            if (known) agent.baselineSessionIds = [...new Set(known)];
        }
        d.agents[agent.id] = agent;
        d.lastAgentType = type;
        const win = agentWindow(agent, cwd);
        d.windows[win.id] = win;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), win.id];
        sess.activeWindowId = win.id;
        attached = true;
    });
    return attached;
}

/**
 * Shows a chat a paired device started among its project's agents, so the Mac
 * can follow it. Leaves the screen where it is: the phone started it, not you.
 */
export function adoptChat(chat: AcpChat): boolean {
    const type = chat.provider;
    if (!isAgentType(type) || !agentSupportsChat(type)) return false;
    let adopted = false;
    mutate((d) => {
        if (d.agents[chat.agentId]) return;
        const sessionId = projectSessionInBackground(d as unknown as StoreState, chat.cwd);
        const permissionMode = normalizePermissionMode(type, chat.permissionMode as AgentPermissionMode);
        const profileId = profileOfLauncher(chat.launcher, d.providerProfiles, type);
        const profile = profileId ? d.providerProfiles.find((item) => item.id === profileId) : undefined;
        const model = chat.model ?? undefined;
        const effort = (chat.effort ?? undefined) as AgentEffort | undefined;
        const resumeId = chat.sessionId ?? undefined;
        const launchOptions = profileLaunchOptions(profile, model, effort);
        const agent: Agent = {
            id: chat.agentId,
            type,
            title: type,
            startup: agentStartup(type, resumeId, permissionMode, profile?.executablePath, launchOptions),
            directCommand: agentDirectCommand(type, resumeId, permissionMode, profile?.executablePath, launchOptions),
            resumeId,
            createdAt: Date.now(),
            permissionMode,
            profileId,
            executablePath: profile?.executablePath,
            cwd: chat.cwd,
            model,
            effort,
            ...(permissionMode === "bypass" ? { skipPermissions: true } : {}),
            launchState: "live",
        };
        d.agents[agent.id] = agent;
        const win = agentWindow(agent, chat.cwd);
        d.windows[win.id] = win;
        d.windowsBySession[sessionId] = [...(d.windowsBySession[sessionId] ?? []), win.id];
        adopted = true;
    });
    return adopted;
}

export function reconcileAgentSessions(type: AgentType, cwd: string, configPath: string | undefined, rows: AgentSession[]): void {
    if (rows.length === 0) return;
    const unsavedNames: { sessionId: string; name: string; executablePath?: string }[] = [];
    mutate((d) => {
        const rowById = new Map(rows.map((row) => [row.id, row]));
        const matchingAgents: Agent[] = [];
        for (const sessionId of d.sessionOrder) {
            const session = d.sessions[sessionId];
            if (session?.kind !== "project") continue;
            for (const agentId of agentIdsOf(d, sessionId)) {
                const agent = d.agents[agentId];
                const agentConfigPath = agent?.profileId
                    ? d.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type)?.configPath
                    : undefined;
                if (agent?.type === type && (agent.cwd || session.cwd) === cwd && agentConfigPath === configPath) matchingAgents.push(agent);
            }
        }
        if (matchingAgents.length === 0) return;

        const claimed = new Set<string>();
        for (const agent of matchingAgents) {
            if (!agent.resumeId) continue;
            claimed.add(agent.resumeId);
            const row = rowById.get(agent.resumeId);
            if (!row) continue;
            if (agent.renamed) {
                if (row.title !== agent.title)
                    unsavedNames.push({ sessionId: row.id, name: agent.title, executablePath: agentExecutablePath(d, agent) });
                continue;
            }
            const nextTitle = usableAgentSessionTitle(row, agent.title);
            if (nextTitle !== agent.title) {
                agent.title = nextTitle;
                const winId = agentWindowId(d, agent.id);
                if (winId) d.windows[winId].name = nextTitle;
            }
        }

        const candidates = rows.filter((row) => !claimed.has(row.id)).sort((a, b) => b.mtime - a.mtime);
        if (candidates.length === 0) return;

        const freshAgents = matchingAgents
            .filter((agent) => !agent.resumeId && d.agentActivity[agent.id]?.source !== "acp")
            .sort((a, b) => (b.createdAt ?? 0) - (a.createdAt ?? 0));
        for (const agent of freshAgents) {
            // Only adopt a session that didn't exist when this agent launched,
            // otherwise it grabs the session you were just in and renames its
            // tab. `baselineSessionIds` is the snapshot taken at creation; when
            // it's missing (legacy agent / cold cache) fall back to "written at
            // or after launch", since a genuinely new session file appears
            // post-launch — never before.
            const baseline = agent.baselineSessionIds;
            const launchedAt = Math.floor((agent.createdAt ?? Date.now()) / 1000);
            const idx = candidates.findIndex((row) => (baseline ? !baseline.includes(row.id) : row.mtime >= launchedAt));
            if (idx < 0) continue;
            const [row] = candidates.splice(idx, 1);
            agent.resumeId = row.id;
            if (agent.renamed) unsavedNames.push({ sessionId: row.id, name: agent.title, executablePath: agentExecutablePath(d, agent) });
            else agent.title = usableAgentSessionTitle(row, agent.title);
            const profile = agent.profileId
                ? d.providerProfiles.find((item) => item.id === agent.profileId && item.provider === agent.type)
                : undefined;
            const launchOptions = profileLaunchOptions(profile, agent.model, agent.effort);
            agent.startup = agentStartup(
                agent.type,
                agent.resumeId,
                agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
                profile?.executablePath || agent.executablePath,
                launchOptions,
            );
            agent.directCommand = agentDirectCommand(
                agent.type,
                agent.resumeId,
                agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
                profile?.executablePath || agent.executablePath,
                launchOptions,
            );
            delete agent.baselineSessionIds;
            claimed.add(row.id);
        }
    });
    for (const { sessionId, name, executablePath } of unsavedNames) {
        void saveSessionName({ type, cwd, sessionId, configPath, executablePath }, name).catch(swallow("save chat name"));
    }
}

export function attachAgentSession(id: string, resumeId: string): void {
    if (!resumeId.trim() || resumeId.length > 4_096 || /[\0\r\n]/.test(resumeId)) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent || agent.resumeId === resumeId) return;
        const profile = agent.profileId
            ? d.providerProfiles.find((candidate) => candidate.id === agent.profileId && candidate.provider === agent.type)
            : undefined;
        const launchOptions = profileLaunchOptions(profile, agent.model, agent.effort);
        const executablePath = profile?.executablePath || agent.executablePath;
        agent.resumeId = resumeId;
        agent.startup = agentStartup(
            agent.type,
            resumeId,
            agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
            executablePath,
            launchOptions,
        );
        agent.directCommand = agentDirectCommand(
            agent.type,
            resumeId,
            agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
            executablePath,
            launchOptions,
        );
        delete agent.baselineSessionIds;
    });
}

/** Moves an agent's working directory, and with it the worktree it belongs to, or none. */
export function setAgentWorktree(id: string, cwd: string, worktree: AgentWorktree | null): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.cwd = cwd;
        if (worktree) agent.worktree = worktree;
        else delete agent.worktree;
        const winId = agentWindowId(d, id);
        const root = winId ? d.windows[winId]?.root : undefined;
        if (root?.type === "pane") root.cwd = cwd;
    });
}

/** Moves a chat that has not started yet into another project, opening the project if it is not open. */
export function moveAgentToProject(id: string, cwd: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        const winId = agentWindowId(d, id);
        const fromId = winId ? ownerSessionId(d, winId) : null;
        if (!agent || !winId || !fromId || agent.resumeId || agent.worktree) return;
        keepOpenedProjectInView(cwd);
        const toId = openProjectSession(d as unknown as StoreState, cwd);
        if (toId === fromId) return;
        const from = d.sessions[fromId];
        const left = d.windowsBySession[fromId].filter((wid) => wid !== winId);
        d.windowsBySession[fromId] = left;
        if (from.activeWindowId === winId) from.activeWindowId = left[left.length - 1] ?? "";
        d.windowsBySession[toId] = [...(d.windowsBySession[toId] ?? []), winId];
        d.sessions[toId].activeWindowId = winId;
        agent.cwd = cwd;
        const root = d.windows[winId].root;
        if (root.type === "pane") root.cwd = cwd;
        const configPath = agent.profileId ? d.providerProfiles.find((profile) => profile.id === agent.profileId)?.configPath : undefined;
        const known = peekResource(agentSessionsR, agent.type, cwd, configPath)?.map((row) => row.id);
        if (known) agent.baselineSessionIds = [...new Set(known)];
        else delete agent.baselineSessionIds;
    });
}

export function setAgentTitle(id: string, title: string): void {
    const value = title.trim();
    if (!value || value.length > 200 || /[\0\r\n]/.test(value)) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (agent && !agent.renamed) agent.title = value;
    });
}

/* Written the way a provider lists a session, so the name reads back unchanged. */
function sessionName(title: string): string | null {
    const name = title.split(/\s+/).filter(Boolean).join(" ");
    if (!name || name.startsWith("<") || [...name].length > SESSION_TITLE_MAX || /\p{Cc}/u.test(name)) return null;
    return name;
}

/** Where a provider keeps a chat, and the CLI that renames it for the providers that rename through their own command. */
interface SavedSession {
    type: AgentType;
    cwd: string;
    sessionId: string;
    configPath?: string;
    executablePath?: string;
}

/* Codex and Hermes start their CLI for every rename, so a name already on its
   way, or one the provider refused, is not sent again. */
const namesInFlight = new Set<string>();
const namesRefused = new Set<string>();

async function saveSessionName(session: SavedSession, name: string): Promise<void> {
    const key = [session.type, session.configPath ?? "", session.sessionId, name].join("\0");
    if (namesInFlight.has(key) || namesRefused.has(key)) return;
    namesInFlight.add(key);
    try {
        await agentApi.renameSession(session.type, session.cwd, session.sessionId, name, session.executablePath, session.configPath);
        invalidate((kind) => kind === "agents.sessions");
    } catch (error) {
        namesRefused.add(key);
        throw error;
    } finally {
        namesInFlight.delete(key);
    }
}

function agentExecutablePath(state: Pick<StoreState, "providerProfiles">, agent: Agent): string | undefined {
    const profile = agent.profileId ? state.providerProfiles.find((item) => item.id === agent.profileId && item.provider === agent.type) : undefined;
    return profile?.executablePath || agent.executablePath;
}

export function renameAgent(id: string, title: string): void {
    const name = sessionName(title);
    if (!name || getState().agents[id]?.title === name) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.title = name;
        agent.renamed = true;
        const winId = agentWindowId(d, id);
        if (winId) d.windows[winId].name = name;
    });
    const state = getState();
    const agent = state.agents[id];
    const winId = agentWindowId(state, id);
    const sessionId = winId ? ownerSessionId(state, winId) : null;
    const cwd = agent?.cwd || (sessionId ? state.sessions[sessionId]?.cwd : undefined);
    if (!agent?.resumeId || !cwd) return;
    const configPath = agent.profileId
        ? state.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type)?.configPath
        : undefined;
    void saveSessionName(
        { type: agent.type, cwd, sessionId: agent.resumeId, configPath, executablePath: agentExecutablePath(state, agent) },
        name,
    ).catch(reportError("rename chat"));
}

/** Renames a chat that is not open, in the provider's own session storage. */
export function renameAgentSession(session: SavedSession, title: string): void {
    const name = sessionName(title);
    if (name) void saveSessionName(session, name).catch(reportError("rename chat"));
}

/* Stands in until the provider titles the conversation, which Claude only does
   once the first turn ends. */
export function titleAgentFromPrompt(id: string, text: string): void {
    const title = text.split(/\s+/).filter(Boolean).join(" ");
    if (!title || title.startsWith("/") || title.startsWith("<")) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent || agent.renamed) return;
        const profile = agent.profileId ? d.providerProfiles.find((item) => item.id === agent.profileId && item.provider === agent.type) : undefined;
        if (agent.title !== (profile?.name || agent.type)) return;
        agent.title = [...title].slice(0, SESSION_TITLE_MAX).join("");
    });
}

export function selectAgent(id: string): void {
    withActiveSession((d, session) => {
        const agent = d.agents[id];
        const winId = agentWindowId(d, id);
        if (!agent || !winId || !(d.windowsBySession[session.id] ?? []).includes(winId)) return;
        const sess = d.sessions[session.id];
        sess.activeWindowId = winId;
        // Picking a real agent tab replaces the draft, exactly like any other tab.
        d.agentPaletteOpen = false;
        if (agent.launchState === "dormant") {
            agent.launchState = "live";
            delete d.agentActivity[id];
            return;
        }
        const activity = d.agentActivity[id];
        if (activity) {
            activity.unread = false;
            if (activity.state === "done") activity.state = "idle";
        }
    });
}

/** Open an agent that lives in some other project, switching to it on the way. */
export function revealAgent(id: string): void {
    const state = getState();
    const windowId = agentWindowId(state, id);
    const sessionId = windowId ? ownerSessionId(state, windowId) : null;
    if (!sessionId) return;
    if (sessionId !== state.activeSessionId) selectSession(sessionId);
    selectAgent(id);
}

export function resumeAgent(id: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.launchState = "live";
        delete d.agentActivity[id];
    });
}

/* A turn is over long before the work it started is. Shells, monitors and
   subagents outlive the answer that launched them, and ending the agent ends
   them too, so the count of what is still going decides whether it can sleep. */
export function noteAgentBackgroundWork(id: string, tasks: number, subagents: number): void {
    mutate((d) => {
        if (!d.agents[id]) return;
        const count = tasks + subagents;
        if (count > 0) d.agentBackgroundWork[id] = count;
        else delete d.agentBackgroundWork[id];
        if (subagents > 0) d.agentSubagents[id] = subagents;
        else delete d.agentSubagents[id];
    });
}

export function agentHasBackgroundWork(state: StoreState, id: string): boolean {
    return (state.agentBackgroundWork[id] ?? 0) > 0;
}

export function sleepAgents(ids: readonly string[]): string[] {
    const sleeping = new Set(ids);
    const slept: string[] = [];
    mutate((d) => {
        for (const id of sleeping) {
            const agent = d.agents[id];
            if (!agent?.resumeId || agent.launchState === "dormant") continue;
            agent.launchState = "dormant";
            delete d.agentBackgroundWork[id];
            delete d.agentSubagents[id];
            slept.push(id);
        }
    });
    return slept;
}

export function sleepAgent(id: string): boolean {
    const agent = getState().agents[id];
    if (!agent?.resumeId) {
        notify("info", "This agent is still establishing its resumable session");
        return false;
    }
    return sleepAgents([id]).length === 1;
}

export function setAgentKeepAlive(id: string, keepAlive: boolean): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        if (keepAlive) agent.keepAlive = true;
        else delete agent.keepAlive;
    });
}

export async function sleepIdleAgents(): Promise<number> {
    const state = getState();
    const ids = Object.values(state.agents)
        .filter(
            (agent) =>
                agent.launchState !== "dormant" &&
                !!agent.resumeId &&
                !agent.keepAlive &&
                !agentHasBackgroundWork(state, agent.id) &&
                state.agentActivity[agent.id]?.backendState === "idle",
        )
        .map((agent) => agent.id);
    const live = await agentIdsWithLiveSessions(state, ids);
    const count = sleepAgents(ids.filter((id) => !live.has(id))).length;
    notify("info", count === 0 ? "No idle resumable agents to sleep" : `Put ${count} idle agent${count === 1 ? "" : "s"} to sleep`);
    return count;
}

export function noteAcpAgentState(id: string, state: import("../types").AgentBackendState): void {
    noteAgentActivity(id, {
        agentId: id,
        state,
        sequence: (getState().agentActivity[id]?.sequence ?? 0) + 1,
        source: "acp",
        confidence: "high",
        reason: "ACP session state",
    });
}

export function noteAgentActivity(id: string, event: "working" | "complete" | import("../agentStatus").AgentStateEvent): void {
    mutate((d) => {
        if (!d.agents[id]) return;
        const visible = activeAgentId(d, d.sessions[d.activeSessionId]) === id;
        const previous = d.agentActivity[id];
        const semantic =
            typeof event === "string"
                ? {
                      agentId: id,
                      state: event === "complete" ? ("idle" as const) : ("working" as const),
                      sequence: (previous?.sequence ?? 0) + 1,
                      source: "activity" as const,
                      confidence: "low" as const,
                      reason: event === "complete" ? "legacy activity settled" : "terminal input or output",
                  }
                : event;
        const reduced = reduceAgentState(previous, semantic, visible);
        if (reduced) d.agentActivity[id] = reduced;
    });
}

export function clearAgentUnread(id: string): void {
    mutate((d) => {
        const activity = d.agentActivity[id];
        if (activity) {
            activity.unread = false;
            if (activity.state === "done") activity.state = "idle";
        }
    });
}

/** An agent closes as its window does; the window's close handles its browser. */
export function closeAgent(id: string): void {
    const winId = agentWindowId(getState(), id);
    if (winId) closeWindowById(winId);
}

export function focusAgents(): void {
    // Agents only exist in project sessions. Other groups (plugins,
    // ssh, command) have no agents and no way back out of "agent" view, so the
    // The agent pane shortcut (⌥4) is a no-op there.
    if (getState().sessions[getState().activeSessionId]?.kind !== "project") return;
    withActiveSession((d, session) => {
        const sess = d.sessions[session.id];
        d.agentRailOpen = true;
        d.zoomedPaneId = null;
        if (d.windows[sess.activeWindowId]?.role === "agent") return;
        const first = (d.windowsBySession[session.id] ?? []).find((id) => d.windows[id]?.role === "agent");
        if (first) sess.activeWindowId = first;
        else d.agentPaletteOpen = true;
    });
    emit({ type: "agent-focus", sessionId: getState().activeSessionId });
}
