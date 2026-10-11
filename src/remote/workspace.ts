import { AGENT_NAMES, CHAT_AGENT_TYPES, agentSupportsChat, normalizePermissionMode } from "../agents/agentLaunch";
import { selectedProviderProfile } from "../agents/agentProfiles";
import { agentCwd } from "../agents/agentPtyContext";
import type { AgentInfo, RecentChat } from "../api/agents";
import type { LauncherRequest, PublishedChat, PublishedProject, PublishedRecent } from "../api/remote";
import { agentWindowId, ownerSessionId } from "../state/selectors";
import type { StoreState } from "../state/store";
import type { AgentPermissionMode, AgentType, ProviderProfile, ProviderProfileSelection, Session } from "../state/types";

/** The longest title the core keeps. */
const MAX_TITLE_CHARS = 200;

const launcherId = (type: AgentType, profile?: ProviderProfile) => (profile ? `${type}:${profile.id}` : type);

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
    catalog: readonly AgentInfo[] = [],
): RemoteWorkspace {
    const statusOf = (type: AgentType, configPath?: string) =>
        catalog.find((agent) => agent.type === type && (agent.configPath ?? undefined) === configPath)?.status?.state;
    const projects = sessionOrder
        .map((id) => sessions[id])
        .filter((session): session is Session => session?.kind === "project" && session.cwd !== "")
        .map((session) => ({ id: session.id, name: session.name, path: session.cwd }));
    const launchers = CHAT_AGENT_TYPES.flatMap((type): LauncherRequest[] => {
        const mode = normalizePermissionMode(type, permissionMode);
        const own = profiles.filter((profile) => profile.provider === type);
        if (own.length === 0)
            return [
                {
                    id: launcherId(type),
                    provider: type,
                    label: AGENT_NAMES[type],
                    environmentKeys: [],
                    permissionMode: mode,
                    status: statusOf(type),
                },
            ];
        return own.map((profile) => ({
            id: launcherId(type, profile),
            provider: type,
            label: own.length > 1 ? `${AGENT_NAMES[type]} · ${profile.name}` : AGENT_NAMES[type],
            configPath: profile.configPath,
            executablePath: profile.executablePath,
            environmentKeys: profile.environmentKeys ?? [],
            permissionMode: mode,
            status: statusOf(type, profile.configPath),
        }));
    });
    return { projects, launchers };
}

/** The provider profile a chat was started with, read back from its launcher's id. */
export function profileOfLauncher(launcher: string | null, profiles: readonly ProviderProfile[], type: AgentType): string | undefined {
    const profileId = launcher?.startsWith(`${type}:`) ? launcher.slice(type.length + 1) : undefined;
    return profiles.some((profile) => profile.id === profileId && profile.provider === type) ? profileId : undefined;
}

/**
 * The rail's recent chats as paired devices list them, each resumed with the launcher of the profile the rail lists it
 * under.
 */
export function remoteRecent(
    chats: readonly RecentChat[],
    profiles: readonly ProviderProfile[],
    selections: ProviderProfileSelection,
): PublishedRecent[] {
    return chats.map((chat) => {
        const own = profiles.filter((profile) => profile.provider === chat.agent);
        const profile = own.length ? (selectedProviderProfile(chat.agent, profiles, selections) ?? own[0]) : undefined;
        return {
            launcher: launcherId(chat.agent, profile),
            provider: chat.agent,
            sessionId: chat.id,
            title: Array.from(chat.title).slice(0, MAX_TITLE_CHARS).join(""),
            cwd: chat.project,
            activeAt: chat.mtime * 1000,
        };
    });
}

/** What the person named each agent, by agent id, for the notch and paired devices to show. */
export function remoteTitles(state: Pick<StoreState, "agents">): Record<string, string> {
    const titles: Record<string, string> = {};
    for (const agent of Object.values(state.agents)) {
        const title = agent.title.trim();
        if (title && title !== agent.type) titles[agent.id] = title;
    }
    return titles;
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
