import { AGENT_NAMES, CHAT_AGENT_TYPES, agentSupportsChat, normalizePermissionMode } from "../agents/agentLaunch";
import { agentCwd } from "../agents/agentPtyContext";
import type { LauncherRequest, PublishedChat, PublishedProject } from "../api/remote";
import { agentWindowId, ownerSessionId } from "../state/selectors";
import type { StoreState } from "../state/store";
import type { AgentPermissionMode, AgentType, ProviderProfile, Session } from "../state/types";

export interface RemoteWorkspace {
    readonly projects: PublishedProject[];
    readonly launchers: LauncherRequest[];
}

/**
 * The projects open in this window and the chat agents it can start, as a
 * paired device sees them. A provider with profiles offers one agent per
 * profile; the others offer one.
 */
export function remoteWorkspace(
    sessions: Record<string, Session>,
    sessionOrder: readonly string[],
    profiles: readonly ProviderProfile[],
    permissionMode: AgentPermissionMode,
): RemoteWorkspace {
    const projects = sessionOrder
        .map((id) => sessions[id])
        .filter((session): session is Session => session?.kind === "project" && session.cwd !== "")
        .map((session) => ({ id: session.id, name: session.name, path: session.cwd }));
    const launchers = CHAT_AGENT_TYPES.flatMap((type): LauncherRequest[] => {
        const mode = normalizePermissionMode(type, permissionMode);
        const own = profiles.filter((profile) => profile.provider === type);
        if (own.length === 0) return [{ id: type, provider: type, label: AGENT_NAMES[type], environmentKeys: [], permissionMode: mode }];
        return own.map((profile) => ({
            id: `${type}:${profile.id}`,
            provider: type,
            label: own.length > 1 ? `${AGENT_NAMES[type]} · ${profile.name}` : AGENT_NAMES[type],
            configPath: profile.configPath,
            executablePath: profile.executablePath,
            environmentKeys: profile.environmentKeys ?? [],
            permissionMode: mode,
        }));
    });
    return { projects, launchers };
}

/** The provider profile a chat was started with, read back from its launcher's id. */
export function profileOfLauncher(launcher: string | null, profiles: readonly ProviderProfile[], type: AgentType): string | undefined {
    const profileId = launcher?.startsWith(`${type}:`) ? launcher.slice(type.length + 1) : undefined;
    return profiles.some((profile) => profile.id === profileId && profile.provider === type) ? profileId : undefined;
}

/** The chat agents in the rail, sleeping ones included, as paired devices list them. */
export function remoteChats(state: Pick<StoreState, "agents" | "windows" | "sessions" | "sessionOrder" | "windowsBySession">): PublishedChat[] {
    return Object.values(state.agents).flatMap((agent) => {
        if (!agentSupportsChat(agent.type)) return [];
        const windowId = agentWindowId(state, agent.id);
        const session = windowId ? state.sessions[ownerSessionId(state, windowId) ?? ""] : undefined;
        const cwd = session && agentCwd(agent, session);
        if (!cwd) return [];
        const named = agent.title.trim() && agent.title !== agent.type;
        return [{ agentId: agent.id, provider: agent.type, title: named ? agent.title : null, cwd, asleep: agent.launchState === "dormant" }];
    });
}
