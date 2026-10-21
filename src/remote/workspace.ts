import { agentLabel, CHAT_AGENT_TYPES } from "../agents/catalog";
import type { AgentType, ProviderProfile } from "../state/types";

/** The launcher the app offers for a provider and profile, as devices and the core name it. */
export const launcherId = (type: AgentType, profileId?: string) => (profileId ? `${type}:${profileId}` : type);

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
