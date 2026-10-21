import { coreCall } from "../api/core";
import type { PinnedChat as CorePinnedChat, ProjectInfo, SpaceInfo, Workspace, WorkspaceAgent } from "../api/generated/wire";
import { getIpcTransport } from "../api/transport";
import { AGENT_PERMISSION_MODES, effectiveMode, normalizePermissionMode } from "../agents/agentLaunch";
import { isAgentType, agentSupportsChat } from "../agents/catalog";
import { toAgentBuddy } from "../buddies/agentBuddy";
import { isRecord } from "../lib/guards";
import { isPluginId } from "../plugins/kinds";
import { dropAgents, dropSession } from "./commands";
import { makeWindow } from "./commands/shared";
import { agentExecutablePath } from "./commands/agentLaunchCommand";
import { agentWindow } from "./agentWindow";
import { isSpaceIcon } from "./projectSpaces";
import { agentIdsOf, agentWindowId, ownerSessionId } from "./selectors";
import { getState, mutate, useStore, type StoreState } from "./store";
import { swallow } from "./toast";
import { allSent, changeOnItsWay } from "./workspaceSent";
import {
    AGENT_EFFORTS,
    type Agent,
    type AgentEffort,
    type AgentPermissionMode,
    type AgentWorktree,
    type PinnedChat,
    type ProjectSpace,
    type Session,
} from "./types";

export const WORKSPACE_CHANGED_EVENT = "workspace_changed";

/** Long enough that one gesture moving several things sends them together. */
const SEND_DELAY_MS = 50;

/** The fields of an agent the core keeps, named the same here and there. Whether it sleeps and when it began are the core's. */
const DURABLE_AGENT_FIELDS = [
    "cwd",
    "title",
    "renamed",
    "lead",
    "resumeId",
    "permissionMode",
    "profileId",
    "model",
    "modelProvider",
    "effort",
    "keepAlive",
    "worktree",
    "buddy",
    "ptyId",
] as const satisfies readonly (keyof WorkspaceAgent & keyof Agent)[];

/** The fields of an agent this window changes. */
const AGENT_FIELDS = ["project", ...DURABLE_AGENT_FIELDS] as const satisfies readonly (keyof WorkspaceAgent)[];

const PROJECT_FIELDS = ["name", "pinned", "space"] as const satisfies readonly (keyof ProjectInfo)[];

type Shared = Pick<Workspace, "projects" | "spaces" | "agents" | "pinned" | "permissionMode" | "disabledPlugins">;

/** The workspace as the core last sent it. */
let known: Workspace | null = null;

export function fetchCoreWorkspace(): Promise<Workspace> {
    return coreCall<Workspace>("workspace");
}

const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

/** The fields of `next` that differ from `was`, a cleared one as null; null when none do. */
function changed<T extends object>(was: T, next: T, fields: readonly (keyof T)[]): Record<string, unknown> | null {
    const patch: Record<string, unknown> = {};
    for (const field of fields) if (!same(was[field], next[field])) patch[field as string] = next[field] ?? null;
    return Object.keys(patch).length ? patch : null;
}

function defined<T extends object>(value: T): T {
    return Object.fromEntries(Object.entries(value).filter(([, field]) => field !== undefined)) as T;
}

export function coreAgentOf(agent: Agent, session: Session, was?: WorkspaceAgent): WorkspaceAgent {
    return defined({
        id: agent.id,
        provider: agent.type,
        project: session.id,
        cwd: agent.cwd ?? session.cwd,
        title: agent.title,
        renamed: agent.renamed === true,
        lead: agent.lead,
        resumeId: agent.resumeId,
        permissionMode: effectiveMode(agent),
        profileId: agent.profileId,
        model: agent.model,
        modelProvider: agent.modelProvider,
        effort: agent.effort,
        keepAlive: agent.keepAlive === true,
        asleep: was?.asleep ?? false,
        worktree: agent.worktree,
        buddy: agent.buddy,
        ptyId: agent.ptyId,
        createdAt: agent.createdAt ?? was?.createdAt ?? 0,
    });
}

const isPermissionMode = (value: string): value is AgentPermissionMode => (AGENT_PERMISSION_MODES as readonly string[]).includes(value);
const isEffort = (value: string | undefined): value is AgentEffort => (AGENT_EFFORTS as readonly (string | undefined)[]).includes(value);

function worktreeOf(value: unknown): AgentWorktree | undefined {
    if (!isRecord(value)) return undefined;
    const { repo, path, branch, base, startSha } = value;
    if (typeof repo !== "string" || typeof path !== "string" || typeof branch !== "string" || typeof startSha !== "string") return undefined;
    return { repo, path, branch, base: typeof base === "string" ? base : null, startSha };
}

/** An agent the core keeps, as the store holds it; null for a CLI this app does not know. */
export function agentFromCore(record: WorkspaceAgent): Agent | null {
    if (!isAgentType(record.provider)) return null;
    const type = record.provider;
    return defined({
        id: record.id,
        type,
        cwd: record.cwd,
        title: record.title,
        renamed: record.renamed || undefined,
        lead: record.lead,
        resumeId: record.resumeId || undefined,
        permissionMode: normalizePermissionMode(type, isPermissionMode(record.permissionMode) ? record.permissionMode : "workspace-write"),
        profileId: record.profileId,
        model: record.model,
        modelProvider: record.modelProvider,
        effort: isEffort(record.effort) ? record.effort : undefined,
        keepAlive: record.keepAlive || undefined,
        worktree: worktreeOf(record.worktree),
        buddy: toAgentBuddy(record.buddy) ?? undefined,
        ptyId: record.ptyId,
        createdAt: record.createdAt,
    });
}

function copyField<K extends (typeof DURABLE_AGENT_FIELDS)[number]>(target: Agent, source: Agent, key: K): void {
    if (source[key] === undefined) delete target[key];
    else target[key] = source[key];
}

function corePinned(chat: PinnedChat): CorePinnedChat {
    return { provider: chat.agent, sessionId: chat.id, title: chat.title, project: chat.project };
}

function localPinned(chat: CorePinnedChat): PinnedChat | null {
    return isAgentType(chat.provider) ? { agent: chat.provider, id: chat.sessionId, title: chat.title, project: chat.project } : null;
}

function localSpace(space: SpaceInfo): ProjectSpace {
    return { id: space.id, name: space.name, icon: isSpaceIcon(space.icon) ? space.icon : "" };
}

/** What the store holds of the workspace's projects, spaces, pins and defaults, as the core sent them. */
export function sharedFromCore(
    workspace: Workspace,
): Pick<StoreState, "spaces" | "projectSpaces" | "codeHosts" | "projectIdentities" | "pinnedChats" | "defaultAgentPermissionMode"> {
    return {
        spaces: workspace.spaces.map(localSpace),
        projectSpaces: Object.fromEntries(workspace.projects.flatMap((project) => (project.space ? [[project.path, project.space]] : []))),
        codeHosts: Object.fromEntries(workspace.projects.map((project) => [project.path, project.codeHosts ?? {}])),
        projectIdentities: Object.fromEntries(workspace.projects.flatMap((project) => (project.identity ? [[project.path, project.identity]] : []))),
        pinnedChats: workspace.pinned.flatMap((chat) => localPinned(chat) ?? []),
        defaultAgentPermissionMode: workspace.permissionMode === "bypass" ? "bypass" : "workspace-write",
    };
}

/** What this window's store holds of the workspace every client shares. */
export function localWorkspace(state: StoreState): Shared {
    const projectSessions = state.sessionOrder
        .map((id) => state.sessions[id])
        .filter((session) => session?.kind === "project" && session.cwd && !session.host);
    const knownAgents = new Map(known?.agents.map((agent) => [agent.id, agent]));
    return {
        projects: projectSessions.map((session) => {
            const space = state.projectSpaces[session.cwd];
            return defined({ id: session.id, name: session.name, path: session.cwd, pinned: session.pinned, space });
        }),
        spaces: state.spaces.map((space) => ({ id: space.id, name: space.name, icon: space.icon })),
        agents: projectSessions.flatMap((session) =>
            agentIdsOf(state, session.id).flatMap((id) => {
                const agent = state.agents[id];
                return agent && !agent.host ? [coreAgentOf(agent, session, knownAgents.get(id))] : [];
            }),
        ),
        pinned: state.pinnedChats.map(corePinned),
        permissionMode: state.defaultAgentPermissionMode,
        disabledPlugins: [...state.disabledPlugins].sort(),
    };
}

/** The core's calls that bring `was` to `next`, in an order the core accepts them in. */
function changes(next: Shared, was: Shared): [string, unknown][] {
    const calls: [string, unknown][] = [];
    if (!same(next.spaces, was.spaces)) calls.push(["setSpaces", { spaces: next.spaces }]);
    const wasProjects = new Map(was.projects.map((project) => [project.id, project]));
    for (const project of next.projects) {
        const before = wasProjects.get(project.id);
        if (!before) calls.push(["addProject", { id: project.id, name: project.name, path: project.path }]);
        const patch = changed(before ?? { ...project, pinned: false, space: undefined }, project, PROJECT_FIELDS);
        if (patch) calls.push(["updateProject", { id: project.id, patch }]);
    }
    const order = next.projects.map((project) => project.id);
    const kept = [...was.projects.map((project) => project.id).filter((id) => order.includes(id)), ...order.filter((id) => !wasProjects.has(id))];
    if (!same(order, kept)) order.forEach((id, index) => calls.push(["moveProject", { id, index }]));
    const nextProjects = new Set(order);
    for (const project of was.projects) if (!nextProjects.has(project.id)) calls.push(["removeProject", { path: project.path }]);
    if (!same(next.pinned, was.pinned)) calls.push(["setPinnedChats", { chats: next.pinned }]);
    if (next.permissionMode !== was.permissionMode) calls.push(["setAgentDefaults", { permissionMode: next.permissionMode }]);
    if (!same(next.disabledPlugins, was.disabledPlugins)) calls.push(["pluginSetDisabled", { ids: next.disabledPlugins }]);
    const wasAgents = new Map(was.agents.map((agent) => [agent.id, agent]));
    for (const agent of next.agents) {
        const before = wasAgents.get(agent.id);
        const patch = before && changed(before, agent, AGENT_FIELDS);
        if (!before) calls.push(["createAgent", { agent }]);
        else if (patch) calls.push(["updateAgent", { agentId: agent.id, patch }]);
    }
    const nextAgents = new Set(next.agents.map((agent) => agent.id));
    const closed = was.agents
        .filter((agent) => !nextAgents.has(agent.id) && nextProjects.has(agent.project) && isAgentType(agent.provider))
        .map((agent) => agent.id);
    if (closed.length) calls.push(["closeAgents", { agentIds: closed }]);
    return calls;
}

function moveWindow(d: StoreState, windowId: string, to: string): void {
    const from = ownerSessionId(d, windowId);
    if (!from || from === to) return;
    d.windowsBySession[from] = d.windowsBySession[from].filter((id) => id !== windowId);
    if (d.sessions[from].activeWindowId === windowId) d.sessions[from].activeWindowId = d.windowsBySession[from][0] ?? "";
    d.windowsBySession[to] = [...(d.windowsBySession[to] ?? []), windowId];
}

/** Brings the store to the workspace the core sent: what another client changed shows here. */
export function applyCoreWorkspace(workspace: Workspace): void {
    known = workspace;
    const state = getState();
    const projects = new Set(workspace.projects.map((project) => project.id));
    const records = new Set(workspace.agents.map((agent) => agent.id));
    const local = localWorkspace(state);
    const goneSessions = local.projects.filter((project) => !projects.has(project.id)).map((project) => project.id);
    const goneAgents = local.agents.filter((agent) => !records.has(agent.id) && projects.has(agent.project)).map((agent) => agent.id);
    mutate((draft) => {
        const d = draft as unknown as StoreState;
        for (const project of workspace.projects) {
            const session = d.sessions[project.id];
            if (session) {
                if (session.kind !== "project") continue;
                session.name = project.name;
                session.cwd = project.path;
                session.pinned = project.pinned;
                continue;
            }
            const window = makeWindow(project.path, "Terminal", { role: "term" });
            d.sessions[project.id] = {
                id: project.id,
                name: project.name,
                kind: "project",
                cwd: project.path,
                pinned: project.pinned,
                activeWindowId: window.id,
            };
            d.sessionOrder.push(project.id);
            d.windows[window.id] = window;
            d.windowsBySession[project.id] = [window.id];
        }
        Object.assign(d, sharedFromCore(workspace));
        if (!same([...d.disabledPlugins].sort(), workspace.disabledPlugins)) d.disabledPlugins = workspace.disabledPlugins.filter(isPluginId);
        for (const record of workspace.agents) {
            const saved = agentFromCore(record);
            const session = d.sessions[record.project];
            if (!saved || session?.kind !== "project") continue;
            const windowId = agentWindowId(d, record.id);
            const current = d.agents[record.id];
            if (current && windowId) {
                const launchState = record.asleep ? "dormant" : "live";
                if (agentSupportsChat(current.type) && current.launchState !== launchState) current.launchState = launchState;
                const owner = d.sessions[ownerSessionId(d, windowId) ?? ""];
                if (owner && !changed(coreAgentOf(current, owner, record), record, AGENT_FIELDS)) continue;
                for (const key of DURABLE_AGENT_FIELDS) copyField(current, saved, key);
                moveWindow(d, windowId, record.project);
                continue;
            }
            const agent: Agent = {
                ...saved,
                executablePath: agentExecutablePath(d, saved),
                createdAt: record.createdAt,
                launchState: record.asleep ? "dormant" : "live",
            };
            d.agents[agent.id] = agent;
            const window = agentWindow(agent, record.cwd);
            d.windows[window.id] = window;
            d.windowsBySession[record.project] = [...(d.windowsBySession[record.project] ?? []), window.id];
        }
    });
    if (goneAgents.length) dropAgents(goneAgents);
    for (const id of goneSessions) dropSession(id);
}

/**
 * Keeps this window and the core's workspace in step: what the person changes here goes to the core, and what the core
 * sends, from any client, comes here. The core is the one source; while a change is on its way, what the core sends is
 * held until it has answered, so a reply from before it never undoes it.
 */
export function startCoreWorkspace(initial: Workspace): () => void {
    applyCoreWorkspace(initial);
    let sending = false;
    let again = false;
    let held: Workspace | null = null;
    let timer: number | undefined;
    const controller = new AbortController();

    const send = async () => {
        if (sending) {
            again = true;
            return;
        }
        sending = true;
        let failed = false;
        do {
            again = false;
            const next = localWorkspace(getState());
            const was = known ?? next;
            for (const [method, params] of changes(next, was)) {
                await coreCall(method, params).catch((error: unknown) => {
                    failed = true;
                    swallow(`change the workspace (${method})`)(error);
                });
            }
            if (known) known = { ...known, ...next };
        } while (again);
        sending = false;
        if (timer === undefined) allSent();
        if (failed) held = await fetchCoreWorkspace().catch(() => held);
        if (held) {
            const workspace = held;
            held = null;
            applyCoreWorkspace(workspace);
        }
    };

    const unsubscribe = useStore.subscribe((state, previous) => {
        if (
            state.sessions === previous.sessions &&
            state.sessionOrder === previous.sessionOrder &&
            state.agents === previous.agents &&
            state.windows === previous.windows &&
            state.windowsBySession === previous.windowsBySession &&
            state.spaces === previous.spaces &&
            state.projectSpaces === previous.projectSpaces &&
            state.pinnedChats === previous.pinnedChats &&
            state.defaultAgentPermissionMode === previous.defaultAgentPermissionMode &&
            state.disabledPlugins === previous.disabledPlugins
        ) {
            return;
        }
        changeOnItsWay();
        window.clearTimeout(timer);
        timer = window.setTimeout(() => {
            timer = undefined;
            void send();
        }, SEND_DELAY_MS);
    });

    getIpcTransport()
        .subscribe<Workspace>(
            WORKSPACE_CHANGED_EVENT,
            ({ payload }) => {
                if (sending || timer !== undefined) held = payload;
                else applyCoreWorkspace(payload);
            },
            { signal: controller.signal },
        )
        .catch((error: unknown) => {
            if (!controller.signal.aborted) swallow("workspace listener")(error);
        });

    return () => {
        controller.abort();
        unsubscribe();
        window.clearTimeout(timer);
        allSent();
    };
}
