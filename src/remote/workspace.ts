import { agentLabel } from "../agents/catalog";
import { CHAT_AGENT_TYPES, agentSupportsChat } from "../agents/agentLaunch";
import { selectedProviderProfile } from "../agents/agentProfiles";
import { agentCwd } from "../agents/agentPtyContext";
import type { RecentChat } from "../api/agents";
import type { PublishedChat, PublishedProject, PublishedRecent } from "../api/remote";
import { agentWindowId, ownerSessionId } from "../state/selectors";
import type { StoreState } from "../state/store";
import type { AgentType, ProviderProfile, ProviderProfileSelection, Session } from "../state/types";

/** The longest title the core keeps. */
const MAX_TITLE_CHARS = 200;

/** The launcher the app offers for a provider and profile, as devices and the core name it. */
export const launcherId = (type: AgentType, profileId?: string) => (profileId ? `${type}:${profileId}` : type);

/** The projects open in this window, as a paired device sees them. */
export function remoteProjects(sessions: Record<string, Session>, sessionOrder: readonly string[]): PublishedProject[] {
    return sessionOrder
        .map((id) => sessions[id])
        .filter((session): session is Session => session?.kind === "project" && session.cwd !== "")
        .map((session) => ({ id: session.id, name: session.name, path: session.cwd }));
}

/** The chat agents the core offers, as its launchers name them: one per profile, or one for a provider with none. */
export function chatLaunchers(profiles: readonly ProviderProfile[]): { id: string; label: string }[] {
    return CHAT_AGENT_TYPES.flatMap((type) => {
        const own = profiles.filter((profile) => profile.provider === type);
        if (own.length === 0) return [{ id: launcherId(type), label: agentLabel(type) }];
        return own.map((profile) => ({
            id: launcherId(type, profile.id),
            label: own.length > 1 ? `${agentLabel(type)} · ${profile.name}` : agentLabel(type),
        }));
    });
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
            launcher: launcherId(chat.agent, profile?.id),
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
