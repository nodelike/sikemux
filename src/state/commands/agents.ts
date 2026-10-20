import { agentApi } from "../../api/agents";
import { acpApi } from "../../api/acp";
import { acpAccount } from "../../agents/accounts";
import { coreCall } from "../../api/core";
import { agentNamesApi } from "../../api/agentNames";
import type { AgentQuestion } from "../../agents/agentQuestions";
import { canBypassPermissions } from "../../agents/catalog";
import {
    agentSupportsChat,
    effectiveMode,
    isResumeId,
    MAX_AGENT_MODEL_LENGTH,
    normalizePermissionMode,
    type ChatAgentType,
} from "../../agents/agentLaunch";
import { selectedAgentRuntimeProfiles } from "../../agents/agentProfiles";
import { projectPermissionMode, projectProfile } from "../../agents/projectDefaults";
import { emit } from "../bus";
import { agentHasBackgroundWork, isAgentBusy, type AgentShown } from "../agentStatus";
import { confirmDialog } from "../dialog";
import { invalidate, peekResource } from "../resources";
import { agentCatalogR } from "../resources.defs";
import { getState, mutate, type StoreState } from "../store";
import { notify, reportError, swallow } from "../toast";
import { agentExecutablePath, profileOf } from "./agentLaunchCommand";
import { activeAgentId, agentIdsOf, agentWindowId, ownerSessionId, projectSessionId } from "../selectors";
import { agentWindow } from "../agentWindow";
import { newId } from "../layout";
import type { Agent, AgentEffort, AgentPermissionMode, AgentType, AgentWorktree } from "../types";
import { selectSession } from "./sessions";
import { retitlePinnedChat } from "./settings";
import { keepOpenedProjectInView, openProjectSession, withActiveSession } from "./shared";
import { closeWindowsById } from "./tabs";

/** Providers list a session under at most this many characters. */
const SESSION_TITLE_MAX = 72;

/** The CLI the agent catalog found for this provider under the selected accounts, once it has looked. */
function detectedExecutablePath(state: StoreState, type: AgentType): string | undefined {
    const runtime = selectedAgentRuntimeProfiles(state.providerProfiles, state.selectedProviderProfileIds);
    return peekResource(agentCatalogR, runtime)?.find((agent) => agent.type === type && agent.available !== false)?.command;
}

export function configureEmptyAgent(id: string, type: ChatAgentType, profileId?: string, model?: string, modelProvider?: string): void {
    const before = getState().agents[id];
    const account = profileId ? profileOf(getState(), { profileId, type }) : undefined;
    /* Its pane starts it again for another agent; another account of the same agent is the core's to move it to. */
    if (before && account && before.type === type && before.profileId !== account.id && inChat(before))
        void acpApi.switchAccount(id, type, acpAccount(account)).catch(reportError("Switch account"));
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent || isAgentBusy(d.agentStatus[id])) return;
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
    });
}

export function setAgentModelPreferences(id: string, model: string | undefined, effort: AgentEffort | undefined): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.model = model;
        agent.effort = effort;
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
    });
}

export function toggleAgentSkipPermissions(id: string): void {
    const agent = getState().agents[id];
    if (!agent || !canBypassPermissions(agent.type)) return;
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
    cwd?: string;
    worktree?: AgentWorktree;
    /** Pin launches to the project that opened the picker. */
    sessionId?: string;
    detectedExecutablePath?: string;
    /** Opens it in a tab behind the one in front. */
    background?: boolean;
}

export function addAgent(type: AgentType, resumeId?: string, title?: string, options: AddAgentOptions = {}): boolean {
    if ((options.model?.trim().length ?? 0) > MAX_AGENT_MODEL_LENGTH) return false;
    if (resumeId && !isResumeId(type, resumeId)) return false;
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
        const permissionMode = normalizePermissionMode(type, options.permissionMode ?? projectPermissionMode(d, session.cwd));
        const requestedProfileId = options.profileId === undefined ? projectProfile(d, session.cwd, type)?.id : options.profileId;
        const profile = requestedProfileId ? profileOf(d, { profileId: requestedProfileId, type }) : undefined;
        const profileId = profile?.id;
        const cwd = options.cwd || session.cwd;
        const model = options.model?.trim() || undefined;
        const executablePath = profile?.executablePath || detected;
        const agent: Agent = {
            id: newId("agent"),
            type,
            title: title ?? type,
            resumeId,
            createdAt: Date.now(),
            permissionMode,
            profileId,
            executablePath,
            cwd,
            ...(options.worktree ? { worktree: options.worktree } : {}),
            model,
            effort: options.effort,
            ...(permissionMode === "bypass" ? { skipPermissions: true } : {}),
            launchState: "live",
        };
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
    /** The worktree of the project the chat ran in, when it ran in one. */
    cwd?: string;
}

interface SavedChatOptions {
    background?: boolean;
    detectedExecutablePath?: string;
}

/** Resumes a saved chat in the project it ran in, back in its worktree if it had one; one already open there comes forward instead. */
export function openSavedChat(chat: SavedChat, options: SavedChatOptions = {}): void {
    const { cwd } = chat;
    if (!cwd) return resumeSavedChat(chat, options);
    void coreCall<AgentWorktree | null>("agentWorktreeAt", { cwd }).then(
        (worktree) => resumeSavedChat(chat, options, cwd, worktree ?? undefined),
        (failure: unknown) => {
            swallow("worktree of a saved chat")(failure);
            resumeSavedChat(chat, options, cwd);
        },
    );
}

function resumeSavedChat(chat: SavedChat, options: SavedChatOptions, cwd?: string, worktree?: AgentWorktree): void {
    const state = getState();
    const project = projectSessionId(state, chat.project) ?? undefined;
    addAgent(chat.agent, chat.id, chat.title, {
        sessionId: project,
        cwd,
        worktree,
        profileId: projectProfile(state, chat.project, chat.agent)?.id,
        detectedExecutablePath: options.detectedExecutablePath,
        background: options.background,
    });
    if (!options.background && project && project !== state.activeSessionId) selectSession(project);
}

/** An agent another agent starts in a terminal, as the core asks the window to open it. */
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
    const shown = state.agentStatus[agent.id];
    return !isAgentBusy(shown) && !agentHasBackgroundWork(shown);
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
    /* A chat's pane starts it again in the new folder once it moves there; a terminal agent sleeps through the move. */
    const terminal = !inChat(agent) && agent.launchState !== "dormant";
    if (resumeId) {
        if (terminal) sleepAgents([id]);
        try {
            await acpApi.stop(id).catch(swallow("stop chat"));
            await agentApi.relocateSession(agent.type, fromCwd, cwd, resumeId, profileOf(state, agent)?.configPath);
        } catch (error) {
            if (terminal) resumeAgent(id);
            reportError("move chat")(error);
            return;
        }
    }
    placeAgentInProject(id, cwd);
    if (resumeId) {
        invalidate((kind) => kind === "agents.sessions");
        if (terminal) resumeAgent(id);
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

export function selectAgent(id: string): void {
    withActiveSession((d, session) => {
        const agent = d.agents[id];
        const winId = agentWindowId(d, id);
        if (!agent || !winId || !(d.windowsBySession[session.id] ?? []).includes(winId)) return;
        const sess = d.sessions[session.id];
        sess.activeWindowId = winId;
        // Picking a real agent tab replaces the draft, exactly like any other tab.
        d.agentPaletteOpen = false;
        if (agent.launchState === "dormant") agent.launchState = "live";
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

/** Whether the agent runs as a chat, whose sleep and waking are the core's, rather than in a terminal. */
function inChat(agent: Agent): boolean {
    return agentSupportsChat(agent.type) && agent.ptyId === undefined;
}

/** Shows a sleeping agent again; its pane starts it, a chat through the core. */
export function resumeAgent(id: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.launchState = "live";
    });
}

export function sleepAgents(ids: readonly string[]): string[] {
    const sleeping = new Set(ids);
    const slept: string[] = [];
    mutate((d) => {
        for (const id of sleeping) {
            const agent = d.agents[id];
            if (!agent?.resumeId || agent.launchState === "dormant") continue;
            agent.launchState = "dormant";
            slept.push(id);
        }
    });
    return slept;
}

/** Puts the agent to sleep; a chat the core finds still at work stays awake. */
export function sleepAgent(id: string): boolean {
    const agent = getState().agents[id];
    if (!agent?.resumeId) {
        notify("info", "This agent is still establishing its resumable session");
        return false;
    }
    if (!inChat(agent)) return sleepAgents([id]).length === 1;
    void acpApi
        .stopIfIdle(id)
        .then((stopped) => {
            if (!stopped) notify("info", "This chat is still at work, so it stays awake");
        })
        .catch(reportError("Sleep chat"));
    return true;
}

export function setAgentKeepAlive(id: string, keepAlive: boolean): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        if (keepAlive) agent.keepAlive = true;
        else delete agent.keepAlive;
    });
}

/** Puts every idle chat to sleep now, through the core. */
export async function sleepIdleAgents(): Promise<number> {
    const { slept } = await coreCall<{ slept: number }>("sleepIdleChats");
    notify("info", slept === 0 ? "No idle chats to sleep" : `Put ${slept} idle chat${slept === 1 ? "" : "s"} to sleep`);
    return slept;
}

/** Takes how the core shows each open agent, from its device view. */
export function setAgentStatus(agents: readonly AgentShown[]): void {
    const before = getState().agentStatus;
    const same = (a: AgentShown, b: AgentShown) =>
        a.status === b.status &&
        a.group === b.group &&
        a.unread === b.unread &&
        a.subagents === b.subagents &&
        a.tasks === b.tasks &&
        a.activeAt === b.activeAt;
    const next = Object.fromEntries(
        agents.map((shown) => [shown.agentId, before[shown.agentId] && same(before[shown.agentId], shown) ? before[shown.agentId] : shown]),
    );
    const ids = Object.keys(next);
    if (ids.length === Object.keys(before).length && ids.every((id) => next[id] === before[id])) return;
    mutate((d) => {
        d.agentStatus = next;
    });
}

/** Takes the open questions the core sent. */
export function setAgentQuestions(questions: AgentQuestion[]): void {
    mutate((d) => {
        d.agentQuestions = questions;
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
