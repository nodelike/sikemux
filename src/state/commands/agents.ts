import { agentApi, type AgentSession } from "../../api/agents";
import { acpApi, type AcpChat } from "../../api/acp";
import { agentNamesApi } from "../../api/agentNames";
import { runOfAgent } from "../../agents/agentBoard";
import { waitsOnPerson, type AgentQuestion } from "../../agents/agentQuestions";
import { profileOfLauncher } from "../../remote/workspace";
import { isAgentType } from "../../agents/catalog";
import { agentSupportsChat, effectiveMode, MAX_AGENT_MODEL_LENGTH, normalizePermissionMode, type ChatAgentType } from "../../agents/agentLaunch";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../../agents/agentProfiles";
import { emit } from "../bus";
import { isAgentBusy, reduceAgentState } from "../agentStatus";
import { confirmDialog } from "../dialog";
import { invalidate, peekResource } from "../resources";
import { agentCatalogR, agentSessionsR } from "../resources.defs";
import { getState, mutate, type StoreState } from "../store";
import { notify, reportError, swallow } from "../toast";
import { agentIdsWithLiveSessions } from "../agentLiveSessions";
import { agentSupportsSkipPermissions } from "./agentLogic";
import { agentExecutablePath, profileOf, relaunchSpec } from "./agentLaunchCommand";
import { activeAgentId, agentIdsOf, agentWindowId, ownerSessionId, projectSessionId } from "../selectors";
import { agentWindow } from "../agentWindow";
import { newId } from "../layout";
import type { Agent, AgentEffort, AgentPermissionMode, AgentType, AgentWorktree, Session } from "../types";
import { selectSession } from "./sessions";
import { retitlePinnedChat } from "./settings";
import { keepOpenedProjectInView, openProjectSession, projectSessionInBackground, withActiveSession } from "./shared";
import { closeWindowsById } from "./tabs";

const FALLBACK_AGENT_TITLE_MAX = 13;
/** Providers list a session under at most this many characters. */
const SESSION_TITLE_MAX = 72;

/** The CLI the agent catalog found for this provider under the selected accounts, once it has looked. */
function detectedExecutablePath(state: StoreState, type: AgentType): string | undefined {
    const runtime = selectedAgentRuntimeProfiles(state.providerProfiles, state.selectedProviderProfileIds);
    return peekResource(agentCatalogR, runtime)?.find((agent) => agent.type === type && agent.available !== false)?.command;
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

/** Moves a chat with nothing in it to another agent, account or model provider.
    `model` picks its model there, on `modelProvider` when one is named. */
export function configureEmptyAgent(id: string, type: ChatAgentType, profileId?: string, model?: string, modelProvider?: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent || isAgentBusy(d.agentActivity[id])) return;
        const profile = profileId ? profileOf(d, { profileId, type }) : undefined;
        if (profileId && !profile) return;
        const mode = normalizePermissionMode(type, agent.permissionMode ?? d.defaultAgentPermissionMode);
        agent.type = type;
        agent.profileId = profile?.id;
        agent.title = profile?.name || type;
        agent.executablePath = profile?.executablePath;
        agent.permissionMode = mode;
        agent.skipPermissions = mode === "bypass";
        delete agent.resumeId;
        agent.model = model;
        agent.modelProvider = modelProvider;
        delete agent.effort;
        delete agent.baselineSessionIds;
        Object.assign(agent, relaunchSpec(d, agent));
    });
}

export function setAgentModelPreferences(id: string, model: string | undefined, effort: AgentEffort | undefined): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.model = model;
        agent.effort = effort;
        Object.assign(agent, relaunchSpec(d, agent));
    });
}

/** Puts the agent on another of the person's accounts, keeping its session: the chat moved there and carries on. */
export function setAgentAccount(id: string, profileId: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        const profile = agent ? profileOf(d, { profileId, type: agent.type }) : undefined;
        if (!agent || !profile || agent.profileId === profile.id) return;
        agent.profileId = profile.id;
        Object.assign(agent, relaunchSpec(d, agent));
    });
}

export function setAgentPermissionMode(id: string, requestedMode: AgentPermissionMode): void {
    mutate((d) => {
        const a = d.agents[id];
        if (!a) return;
        const next = normalizePermissionMode(a.type, requestedMode);
        if (next === effectiveMode(a)) return;
        a.permissionMode = next;
        a.skipPermissions = next === "bypass";
        Object.assign(a, relaunchSpec(d, a));
    });
}

export function toggleAgentSkipPermissions(id: string): void {
    const agent = getState().agents[id];
    if (!agent || !agentSupportsSkipPermissions(agent.type)) return;
    const next = effectiveMode(agent) === "bypass" ? normalizePermissionMode(agent.type, "workspace-write") : "bypass";
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
    /** Opens it in a tab behind the one in front. */
    background?: boolean;
}

export function addAgent(type: AgentType, resumeId?: string, title?: string, options: AddAgentOptions = {}): boolean {
    if ((options.model?.trim().length ?? 0) > MAX_AGENT_MODEL_LENGTH) return false;
    const detected = options.detectedExecutablePath ?? detectedExecutablePath(getState(), type);
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
        attached = true;
        if (!options.background) {
            d.zoomedPaneId = null;
            // A successful launch closes the picker and activates the new PTY.
            d.agentPaletteOpen = false;
        }
        if (existing) {
            const winId = agentWindowId(d, existing.id);
            if (winId && !options.background) sess.activeWindowId = winId;
            return;
        }
        const permissionMode = normalizePermissionMode(type, options.permissionMode ?? d.defaultAgentPermissionMode);
        const requestedProfileId = options.profileId === undefined ? d.selectedProviderProfileIds[type] : options.profileId;
        const profile = requestedProfileId ? profileOf(d, { profileId: requestedProfileId, type }) : undefined;
        const profileId = profile?.id;
        const cwd = options.cwd || session.cwd;
        const model = options.model?.trim() || undefined;
        const executablePath = profile?.executablePath || detected;
        const agent: Agent = {
            id: newId("agent"),
            type,
            title: title ?? type,
            ...relaunchSpec(d, { type, resumeId, permissionMode, profileId, executablePath, model, effort: options.effort }),
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
        if (!options.background) sess.activeWindowId = win.id;
    });
    return attached;
}

/** A session an agent CLI saved, by its id, in the project it ran in. */
export interface SavedChat {
    agent: AgentType;
    id: string;
    title: string;
    project: string;
}

/** Resumes a saved chat in the project it ran in; one already open there comes forward instead. */
export function openSavedChat(chat: SavedChat, options: { background?: boolean; detectedExecutablePath?: string } = {}): void {
    const state = getState();
    const project = projectSessionId(state, chat.project) ?? undefined;
    addAgent(chat.agent, chat.id, chat.title, {
        sessionId: project,
        profileId: selectedProviderProfile(chat.agent, state.providerProfiles, state.selectedProviderProfileIds)?.id,
        detectedExecutablePath: options.detectedExecutablePath,
        background: options.background,
    });
    if (!options.background && project && project !== state.activeSessionId) selectSession(project);
}

/**
 * Shows a chat a paired device or another agent started among its project's agents, so the host can follow it.
 * Leaves the screen where it is: the person did not start it.
 */
export function adoptChat(chat: AcpChat): boolean {
    const type = chat.provider;
    if (!isAgentType(type) || !agentSupportsChat(type)) return false;
    let adopted = false;
    mutate((d) => {
        if (d.agents[chat.agentId]) return;
        const lead = chat.startedBy ? d.agents[chat.startedBy] : undefined;
        const leadWindowId = lead ? agentWindowId(d, lead.id) : null;
        const auto = lead ? null : runOfAgent(d.agentBoards, chat.agentId);
        const sessionId =
            (leadWindowId && ownerSessionId(d, leadWindowId)) || projectSessionInBackground(d as unknown as StoreState, auto?.project ?? chat.cwd);
        const leadTree = lead?.worktree;
        const run = auto?.card.auto;
        const worktree: AgentWorktree | undefined = run
            ? { repo: run.repo, path: run.worktree, branch: run.branch, base: run.base, startSha: run.startSha }
            : leadTree && (chat.cwd === leadTree.path || chat.cwd.startsWith(`${leadTree.path}/`))
              ? { ...leadTree }
              : undefined;
        const permissionMode = normalizePermissionMode(type, chat.permissionMode as AgentPermissionMode);
        const profileId = profileOfLauncher(chat.launcher, d.providerProfiles, type) ?? (lead?.type === type ? lead.profileId : undefined);
        const profile = profileOf(d, { profileId, type });
        const model = chat.model ?? undefined;
        const effort = (chat.effort ?? undefined) as AgentEffort | undefined;
        const resumeId = chat.sessionId ?? undefined;
        const agent: Agent = {
            id: chat.agentId,
            type,
            ...(chat.name ? { name: chat.name } : {}),
            ...(lead ? { lead: lead.id } : {}),
            title: type,
            ...relaunchSpec(d, { type, resumeId, permissionMode, profileId, executablePath: profile?.executablePath, model, effort }),
            resumeId,
            createdAt: Date.now(),
            permissionMode,
            profileId,
            executablePath: profile?.executablePath,
            cwd: chat.cwd,
            model,
            effort,
            ...(worktree ? { worktree } : {}),
            ...(chat.modelProvider ? { modelProvider: chat.modelProvider } : {}),
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

/** An agent another agent starts in a terminal, as the core asks the window to open it. */
export interface TerminalWorker {
    agentId: string;
    cli: string;
    name: string;
    lead: string;
    model?: string | null;
    permissionMode: string;
    launcher?: string | null;
}

/**
 * The record of a terminal worker, in its lead's folder and on its lead's account when they run the same CLI, and the
 * project tab it joins: its lead's. Null when the lead is no longer open.
 */
export function planTerminalWorker(worker: TerminalWorker): { agent: Agent; session: Session } | null {
    const state = getState();
    const type = worker.cli;
    const lead = state.agents[worker.lead];
    const leadWindowId = lead ? agentWindowId(state, lead.id) : null;
    const sessionId = leadWindowId ? ownerSessionId(state, leadWindowId) : null;
    const session = sessionId ? state.sessions[sessionId] : undefined;
    if (!isAgentType(type) || !lead || !session || session.kind !== "project" || state.agents[worker.agentId]) return null;
    const same = lead.type === type;
    const cwd = lead.cwd || session.cwd;
    const leadTree = lead.worktree;
    const worktree = leadTree && (cwd === leadTree.path || cwd.startsWith(`${leadTree.path}/`)) ? { ...leadTree } : undefined;
    const permissionMode = normalizePermissionMode(type, worker.permissionMode as AgentPermissionMode);
    const profileId =
        profileOfLauncher(worker.launcher ?? null, state.providerProfiles, type) ?? (same ? lead.profileId : state.selectedProviderProfileIds[type]);
    const profile = profileOf(state, { profileId, type });
    const executablePath = profile?.executablePath || (same ? lead.executablePath : undefined) || detectedExecutablePath(state, type);
    const model = worker.model ?? (same ? lead.model : undefined);
    const effort = same ? lead.effort : undefined;
    const known = peekResource(agentSessionsR, type, cwd, profile?.configPath)?.map((row) => row.id);
    const agent: Agent = {
        id: worker.agentId,
        type,
        name: worker.name,
        lead: lead.id,
        title: type,
        ...relaunchSpec(state, { type, permissionMode, profileId: profile?.id, executablePath, model, effort }),
        createdAt: Date.now(),
        permissionMode,
        profileId: profile?.id,
        executablePath,
        cwd,
        model,
        effort,
        ...(known ? { baselineSessionIds: [...new Set(known)] } : {}),
        ...(worktree ? { worktree } : {}),
        ...(permissionMode === "bypass" ? { skipPermissions: true } : {}),
        launchState: "live",
    };
    return { agent, session };
}

/** Shows a terminal worker already running in its terminal beside its lead, leaving the screen where it is. */
export function adoptTerminalWorker(agent: Agent, sessionId: string): boolean {
    let adopted = false;
    mutate((d) => {
        const session = d.sessions[sessionId];
        if (d.agents[agent.id] || !session) return;
        d.agents[agent.id] = agent;
        const win = agentWindow(agent, agent.cwd || session.cwd);
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
                const agentConfigPath = agent ? profileOf(d, agent)?.configPath : undefined;
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
            Object.assign(agent, relaunchSpec(d, agent));
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
        agent.resumeId = resumeId;
        Object.assign(agent, relaunchSpec(d, agent));
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

/** Providers whose saved chat can be filed under another project once it has started. */
const RELOCATABLE_SESSIONS: ReadonlySet<AgentType> = new Set(["claude", "codex"]);

/** Whether the agent can move to another project now: before its first message, or while a Claude or Codex chat sits idle. */
export function canMoveAgentToProject(state: StoreState, agent: Agent): boolean {
    if (agent.worktree) return false;
    if (!agent.resumeId) return true;
    if (!RELOCATABLE_SESSIONS.has(agent.type)) return false;
    if (agent.launchState === "dormant") return true;
    const backend = state.agentActivity[agent.id]?.backendState;
    return backend !== "working" && backend !== "blocked" && !agentHasBackgroundWork(state, agent.id);
}

/**
 * Moves an agent into another project, opening the project if it is not open.
 * A started chat is stopped, its saved session filed under the new folder, then resumed there.
 */
export async function moveAgentToProject(id: string, cwd: string): Promise<void> {
    const state = getState();
    const agent = state.agents[id];
    const winId = agentWindowId(state, id);
    const fromId = winId ? ownerSessionId(state, winId) : null;
    if (!agent || !fromId || !canMoveAgentToProject(state, agent)) return;
    const fromCwd = agent.cwd || state.sessions[fromId].cwd;
    if (fromCwd === cwd) return;
    const resumeId = agent.resumeId;
    const awake = agent.launchState !== "dormant";
    if (resumeId) {
        sleepAgents([id]);
        try {
            await acpApi.stop(id).catch(swallow("stop chat"));
            await agentApi.relocateSession(agent.type, fromCwd, cwd, resumeId, profileOf(state, agent)?.configPath);
        } catch (error) {
            if (awake) resumeAgent(id);
            reportError("move chat")(error);
            return;
        }
    }
    placeAgentInProject(id, cwd);
    if (resumeId) {
        invalidate((kind) => kind === "agents.sessions");
        if (awake) resumeAgent(id);
    }
}

function placeAgentInProject(id: string, cwd: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        const winId = agentWindowId(d, id);
        const fromId = winId ? ownerSessionId(d, winId) : null;
        if (!agent || !winId || !fromId) return;
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
        if (agent.resumeId) return;
        const configPath = profileOf(d, agent)?.configPath;
        const known = peekResource(agentSessionsR, agent.type, cwd, configPath)?.map((row) => row.id);
        if (known) agent.baselineSessionIds = [...new Set(known)];
        else delete agent.baselineSessionIds;
    });
}

/** Takes up the name the core gave the agent, and the agent that started it, if one did. */
export function setAgentName(id: string, name: string, lead: string | null = null): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        if (agent.name !== name) agent.name = name;
        if (lead && agent.lead !== lead) agent.lead = lead;
        else if (!lead && agent.lead) delete agent.lead;
    });
}

/** The person's own name for the agent. The core refuses one another open agent of the project already has. */
export async function changeAgentName(id: string, name: string): Promise<void> {
    const named = await agentNamesApi.rename(id, name.trim());
    setAgentName(id, named.name, named.parent);
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
    if (agent?.resumeId) retitlePinnedChat(agent.type, agent.resumeId, name);
    const winId = agentWindowId(state, id);
    const sessionId = winId ? ownerSessionId(state, winId) : null;
    const cwd = agent?.cwd || (sessionId ? state.sessions[sessionId]?.cwd : undefined);
    if (!agent?.resumeId || !cwd) return;
    const configPath = profileOf(state, agent)?.configPath;
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

/** Deletes a chat that is not open from the provider's own session storage, once the person confirms. */
export async function deleteAgentSession(session: SavedSession, title: string): Promise<boolean> {
    const confirmed = await confirmDialog({
        title: "Delete this chat?",
        body: `“${title}” will be removed from disk for good and can no longer be resumed.`,
        confirmLabel: "Delete",
        destructive: true,
    });
    if (!confirmed) return false;
    try {
        await agentApi.deleteSession(session.type, session.cwd, session.sessionId, session.executablePath, session.configPath);
    } catch (error) {
        reportError("delete chat")(error);
        return false;
    }
    invalidate((kind) => kind === "agents.sessions");
    return true;
}

/* Stands in until the provider titles the conversation, which Claude only does
   once the first turn ends. */
export function titleAgentFromPrompt(id: string, text: string): void {
    const title = text.split(/\s+/).filter(Boolean).join(" ");
    if (!title || title.startsWith("/") || title.startsWith("<")) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent || agent.renamed) return;
        if (agent.title !== (profileOf(d, agent)?.name || agent.type)) return;
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
        if (tasks > 0) d.agentTasks[id] = tasks;
        else delete d.agentTasks[id];
        if (subagents > 0) d.agentSubagents[id] = subagents;
        else delete d.agentSubagents[id];
    });
}

export function agentHasBackgroundWork(state: StoreState, id: string): boolean {
    return (state.agentTasks[id] ?? 0) + (state.agentSubagents[id] ?? 0) > 0;
}

/**
 * Whether an agent can be put to sleep without losing anything: it can be resumed, nothing it
 * started is still running, and it is surely idle rather than guessed idle from a quiet terminal.
 */
export function isAgentSleepable(state: StoreState, agent: Agent): boolean {
    const activity = state.agentActivity[agent.id];
    return (
        agent.launchState !== "dormant" &&
        !!agent.resumeId &&
        !agent.keepAlive &&
        !agentHasBackgroundWork(state, agent.id) &&
        activity?.backendState === "idle" &&
        activity.confidence !== "low"
    );
}

export function sleepAgents(ids: readonly string[]): string[] {
    const sleeping = new Set(ids);
    const slept: string[] = [];
    mutate((d) => {
        for (const id of sleeping) {
            const agent = d.agents[id];
            if (!agent?.resumeId || agent.launchState === "dormant") continue;
            agent.launchState = "dormant";
            delete d.agentTasks[id];
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
        .filter((agent) => isAgentSleepable(state, agent))
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
        if (semantic.state !== "stopped" && waitsOnPerson(d.agentQuestions, id)) {
            if (previous && semantic.sequence <= previous.sequence) return;
            const held = previous?.reported !== undefined ? previous : reduceAgentState(previous, { ...semantic, state: "blocked" }, visible);
            if (held) d.agentActivity[id] = { ...held, sequence: semantic.sequence, reported: semantic.state };
            return;
        }
        const reduced = reduceAgentState(previous, semantic, visible);
        if (reduced) d.agentActivity[id] = reduced;
    });
}

/**
 * Takes the open questions the core sent. An agent waiting on the person's answer shows as needing input whatever it
 * reports, and goes back to what it last reported once the question is answered.
 */
export function setAgentQuestions(questions: AgentQuestion[]): void {
    mutate((d) => {
        const before = d.agentQuestions;
        d.agentQuestions = questions;
        for (const id of new Set([...before, ...questions].map((question) => question.agentId))) {
            const asking = waitsOnPerson(questions, id);
            if (asking === waitsOnPerson(before, id) || !d.agents[id]) continue;
            const previous = d.agentActivity[id];
            if (asking) {
                const visible = activeAgentId(d, d.sessions[d.activeSessionId]) === id;
                const held = reduceAgentState(
                    previous,
                    {
                        agentId: id,
                        state: "blocked",
                        sequence: (previous?.sequence ?? 0) + 1,
                        source: previous?.source ?? "activity",
                        confidence: "high",
                        reason: "asked the person a question",
                    },
                    visible,
                );
                if (held) d.agentActivity[id] = { ...held, reported: previous?.backendState ?? "idle" };
            } else if (previous?.reported !== undefined) {
                const { reported, ...rest } = previous;
                d.agentActivity[id] = { ...rest, state: reported, backendState: reported, unread: false, reason: "question answered" };
            }
        }
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

/** An agent closes as its window does; the window's close handles its browser and asks first if the agent is busy. */
export function closeAgent(id: string): void {
    closeAgents([id]);
}

export function closeAgents(ids: readonly string[]): void {
    const state = getState();
    closeWindowsById(ids.flatMap((id) => agentWindowId(state, id) ?? []));
}

/** Closes agents the core no longer has, without asking: there is nothing left running to stop. */
export function dropAgents(ids: readonly string[]): void {
    const state = getState();
    closeWindowsById(
        ids.flatMap((id) => agentWindowId(state, id) ?? []),
        false,
    );
}

export function focusAgents(): void {
    // Agents only exist in project sessions. Other groups (plugins,
    // ssh, command) have no agents and no way back out of "agent" view, so the
    // The agent pane shortcut (⌥4) is a no-op there.
    if (getState().sessions[getState().activeSessionId]?.kind !== "project") return;
    withActiveSession((d, session) => {
        const sess = d.sessions[session.id];
        d.tabRailOpen = true;
        d.zoomedPaneId = null;
        if (d.windows[sess.activeWindowId]?.role === "agent") return;
        const first = (d.windowsBySession[session.id] ?? []).find((id) => d.windows[id]?.role === "agent");
        if (first) sess.activeWindowId = first;
        else d.agentPaletteOpen = true;
    });
    emit({ type: "agent-focus", sessionId: getState().activeSessionId });
}
